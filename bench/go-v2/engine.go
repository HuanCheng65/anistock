package main

import (
	"encoding/json"
	"math"
	"math/rand"
	"time"
)

const (
	numSyms     = 141
	maxAcct     = 11140
	npcAcctLo   = 10000
	npcAcctN    = 1000
	mmAcctBase  = 11000
	startCash   = int64(1_000_000_000_000)
	startShares = int64(1_000_000)
	maxQty      = 100000
	npcTTL      = 30 * time.Second
	mmQty       = 200
	depthLevels = 5
)

type account struct {
	cash, cashFrozen int64
}

type holding struct {
	shares, sharesFrozen int64
}

type npcEntry struct {
	placedAt time.Time
	id       int64
}

// Sink receives log lines produced by the engine. The engine never blocks on it.
type sink interface {
	logLine(line []byte)
}

type orderReq struct {
	Acct int64  `json:"acct"`
	Sym  int64  `json:"sym"`
	Side string `json:"side"`
	Type string `json:"type"`
	Qty  int64  `json:"qty"`
	Px   int64  `json:"px"`
}

type orderResp struct {
	ID     int64  `json:"id"`
	Filled int64  `json:"filled"`
	Status string `json:"status"`
}

type stats struct {
	Orders  int64 `json:"orders"`
	NPC     int64 `json:"npc"`
	Trades  int64 `json:"trades"`
	WS      int64 `json:"ws"`
	Dropped int64 `json:"dropped"`
}

type engine struct {
	base     [numSyms]int64
	last     [numSyms]int64
	vol      [numSyms]int64
	books    [numSyms]*book
	accounts []account
	holdings map[int64]*holding
	open     map[int64]*order
	nextID   int64

	st stats

	npcFIFO []npcEntry
	npcHead int
	mmBid   [numSyms]int64
	mmAsk   [numSyms]int64
	// v2 conflation: symbols touched by an order/cancel since the last book flush.
	dirty     [numSyms]bool
	dirtyList []int
	// v2 snapshot: last/vol as of the previous mkt snapshot.
	snapLast [numSyms]int64
	snapVol  [numSyms]int64

	rng      *rand.Rand
	sink     sink
	nowMicro func() int64
	now      func() time.Time
}

func newEngine(s sink) *engine {
	e := &engine{
		accounts: make([]account, maxAcct+1),
		holdings: make(map[int64]*holding),
		open:     make(map[int64]*order),
		nextID:   1,
		rng:      rand.New(rand.NewSource(time.Now().UnixNano())),
		sink:     s,
		now:      time.Now,
	}
	e.nowMicro = func() int64 { return e.now().UnixMicro() }
	for s := 0; s < numSyms; s++ {
		e.base[s] = 1000 + 100*int64(s)
		e.last[s] = e.base[s]
		e.snapLast[s] = e.base[s]
		e.books[s] = newBook()
	}
	for i := range e.accounts {
		e.accounts[i].cash = startCash
	}
	return e
}

func (e *engine) holding(acct int64, sym int) *holding {
	k := acct*numSyms + int64(sym)
	h := e.holdings[k]
	if h == nil {
		h = &holding{shares: startShares}
		e.holdings[k] = h
	}
	return h
}

type rejectErr string

func (r rejectErr) Error() string { return string(r) }

// placeOrder validates, matches and (for limits) rests an order. It emits the
// "o" and "t" log lines and one book event. Rejected orders emit nothing.
func (e *engine) placeOrder(r *orderReq) (orderResp, error) {
	if r.Acct < 0 || r.Acct > maxAcct {
		return orderResp{}, rejectErr("bad acct")
	}
	if r.Sym < 0 || r.Sym >= numSyms {
		return orderResp{}, rejectErr("bad sym")
	}
	if r.Qty < 1 || r.Qty > maxQty {
		return orderResp{}, rejectErr("bad qty")
	}
	var buy, market bool
	switch r.Side {
	case "B":
		buy = true
	case "S":
	default:
		return orderResp{}, rejectErr("bad side")
	}
	switch r.Type {
	case "L":
	case "M":
		market = true
	default:
		return orderResp{}, rejectErr("bad type")
	}
	sym := int(r.Sym)
	last := e.last[sym]
	px := int64(0)
	if !market {
		px = r.Px
		if px < last/2 || px > last*2 {
			return orderResp{}, rejectErr("price out of band")
		}
	}
	acc := &e.accounts[r.Acct]
	var pReserve int64
	if buy {
		if market {
			pReserve = last * 2
		} else {
			pReserve = px
		}
		reserve := r.Qty * pReserve
		if acc.cash-acc.cashFrozen < reserve {
			return orderResp{}, rejectErr("insufficient cash")
		}
		acc.cashFrozen += reserve
	} else {
		h := e.holding(r.Acct, sym)
		if h.shares-h.sharesFrozen < r.Qty {
			return orderResp{}, rejectErr("insufficient shares")
		}
		h.sharesFrozen += r.Qty
	}

	ts := e.nowMicro()
	o := &order{
		id: e.nextID, acct: r.Acct, sym: sym, buy: buy, market: market,
		qty: r.Qty, px: px, pReserve: pReserve, ts: ts,
	}
	e.nextID++
	e.logOrder(o)

	bk := e.books[sym]
	var opp *side
	if buy {
		opp = &bk.asks
	} else {
		opp = &bk.bids
	}
	stp := false
	for o.remaining() > 0 {
		lv := opp.best()
		if lv == nil {
			break
		}
		if !market {
			if buy && lv.px > px {
				break
			}
			if !buy && lv.px < px {
				break
			}
		}
		rest := lv.head
		if rest.acct == o.acct {
			stp = true
			break
		}
		q := o.remaining()
		if rr := rest.remaining(); rr < q {
			q = rr
		}
		p := rest.px
		if buy {
			e.settle(o, rest, q, p)
		} else {
			e.settle(rest, o, q, p)
		}
		o.filled += q
		rest.filled += q
		lv.total -= q
		if rest.remaining() == 0 {
			opp.remove(rest)
			delete(e.open, rest.id)
			e.release(rest) // nothing left, but keeps the rule in one place
		}
	}

	var status string
	switch {
	case o.remaining() == 0:
		status = "filled"
	case stp || market:
		status = "cancelled"
		e.release(o)
	default:
		status = "open"
		if buy {
			bk.bids.add(o)
		} else {
			bk.asks.add(o)
		}
		e.open[o.id] = o
	}
	e.markDirty(sym)
	return orderResp{ID: o.id, Filled: o.filled, Status: status}, nil
}

func (e *engine) settle(b, s *order, q, p int64) {
	sym := b.sym
	ba := &e.accounts[b.acct]
	ba.cash -= q * p
	ba.cashFrozen -= q * b.pReserve
	sa := &e.accounts[s.acct]
	sa.cash += q * p
	sh := e.holding(s.acct, sym)
	sh.shares -= q
	sh.sharesFrozen -= q
	e.holding(b.acct, sym).shares += q
	e.last[sym] = p
	e.vol[sym] += q
	e.st.Trades++
	e.logTrade(sym, p, q, b.id, s.id)
}

// release frees whatever is still frozen for an order that has ended.
func (e *engine) release(o *order) {
	rem := o.remaining()
	if rem == 0 {
		return
	}
	if o.buy {
		e.accounts[o.acct].cashFrozen -= rem * o.pReserve
	} else {
		e.holding(o.acct, o.sym).sharesFrozen -= rem
	}
}

// cancel cancels an open order owned by acct. Emits a "c" line and a book event on success.
func (e *engine) cancel(acct, id int64) bool {
	o := e.open[id]
	if o == nil || o.acct != acct {
		return false
	}
	bk := e.books[o.sym]
	o.lvl.total -= o.remaining()
	if o.buy {
		bk.bids.remove(o)
	} else {
		bk.asks.remove(o)
	}
	delete(e.open, id)
	e.release(o)
	e.logCancel(id)
	e.markDirty(o.sym)
	return true
}

func (e *engine) ref(sym int) int64 {
	return (8*e.last[sym] + 2*e.base[sym]) / 10
}

// npcTick expires old NPC limit orders, then places rate/10 NPC orders.
func (e *engine) npcTick(perTick int) {
	now := e.now()
	for e.npcHead < len(e.npcFIFO) {
		ent := e.npcFIFO[e.npcHead]
		if now.Sub(ent.placedAt) < npcTTL {
			break
		}
		e.npcFIFO[e.npcHead] = npcEntry{}
		e.npcHead++
		if o := e.open[ent.id]; o != nil {
			e.cancel(o.acct, ent.id)
		}
	}
	if e.npcHead > 4096 && e.npcHead*2 > len(e.npcFIFO) {
		n := copy(e.npcFIFO, e.npcFIFO[e.npcHead:])
		e.npcFIFO = e.npcFIFO[:n]
		e.npcHead = 0
	}

	for i := 0; i < perTick; i++ {
		r := orderReq{
			Acct: npcAcctLo + int64(e.rng.Intn(npcAcctN)),
			Sym:  int64(e.rng.Intn(numSyms)),
		}
		if e.rng.Intn(2) == 0 {
			r.Side = "B"
		} else {
			r.Side = "S"
		}
		ref := e.ref(int(r.Sym))
		if e.rng.Float64() < 0.8 {
			r.Type = "L"
			u := e.rng.Float64()*0.04 - 0.02
			r.Px = int64(math.Round(float64(ref) * (1 + u)))
			r.Qty = 1 + int64(e.rng.Intn(100))
		} else {
			r.Type = "M"
			r.Qty = 1 + int64(e.rng.Intn(50))
		}
		resp, err := e.placeOrder(&r)
		if err != nil {
			continue
		}
		e.st.NPC++
		if r.Type == "L" && resp.Status == "open" {
			e.npcFIFO = append(e.npcFIFO, npcEntry{placedAt: now, id: resp.ID})
		}
	}
}

// mmTick re-quotes the market makers of every symbol with sym % 10 == k % 10
// (staggered: tick k is the k-th NPC tick, so each symbol is re-quoted once per second).
func (e *engine) mmTick(k int) {
	for s := k % 10; s < numSyms; s += 10 {
		acct := int64(mmAcctBase + s)
		if e.mmBid[s] != 0 {
			e.cancel(acct, e.mmBid[s])
			e.mmBid[s] = 0
		}
		if e.mmAsk[s] != 0 {
			e.cancel(acct, e.mmAsk[s])
			e.mmAsk[s] = 0
		}
		ref := e.ref(s)
		bid := orderReq{Acct: acct, Sym: int64(s), Side: "B", Type: "L", Qty: mmQty, Px: ref * 99 / 100}
		if resp, err := e.placeOrder(&bid); err == nil {
			e.st.NPC++
			e.mmBid[s] = resp.ID
		}
		ask := orderReq{Acct: acct, Sym: int64(s), Side: "S", Type: "L", Qty: mmQty, Px: ref * 101 / 100}
		if resp, err := e.placeOrder(&ask); err == nil {
			e.st.NPC++
			e.mmAsk[s] = resp.ID
		}
	}
}

// ---- wire encoding ----

type bookMsg struct {
	T    string     `json:"t"`
	Sym  int        `json:"sym"`
	Last int64      `json:"last"`
	Vol  int64      `json:"vol"`
	Bids [][2]int64 `json:"bids"`
	Asks [][2]int64 `json:"asks"`
	TS   int64      `json:"ts"`
}

type mktMsg struct {
	T  string     `json:"t"`
	TS int64      `json:"ts"`
	S  [][3]int64 `json:"s"`
}

// markDirty records that sym's book changed; the book message is sent at the next flush.
func (e *engine) markDirty(sym int) {
	if !e.dirty[sym] {
		e.dirty[sym] = true
		e.dirtyList = append(e.dirtyList, sym)
	}
}

// flushDirty calls send(sym) for every dirty symbol (in the order they became dirty)
// and clears all dirty flags.
func (e *engine) flushDirty(send func(sym int)) {
	for _, sym := range e.dirtyList {
		e.dirty[sym] = false
		send(sym)
	}
	e.dirtyList = e.dirtyList[:0]
}

// bookMessage encodes the current top-of-book of sym (ts = time of encoding).
func (e *engine) bookMessage(sym int) []byte {
	bk := e.books[sym]
	m := bookMsg{
		T: "book", Sym: sym, Last: e.last[sym], Vol: e.vol[sym],
		Bids: bk.bids.depth(depthLevels), Asks: bk.asks.depth(depthLevels),
		TS: e.nowMicro(),
	}
	b, _ := json.Marshal(&m)
	return b
}

// changedSnapshot encodes a mkt message with only the symbols whose last or vol
// changed since the previous changedSnapshot, and records the new baseline.
func (e *engine) changedSnapshot() []byte {
	m := mktMsg{T: "mkt", TS: e.nowMicro(), S: make([][3]int64, 0, numSyms)}
	for s := 0; s < numSyms; s++ {
		if e.last[s] != e.snapLast[s] || e.vol[s] != e.snapVol[s] {
			m.S = append(m.S, [3]int64{int64(s), e.last[s], e.vol[s]})
			e.snapLast[s] = e.last[s]
			e.snapVol[s] = e.vol[s]
		}
	}
	b, _ := json.Marshal(&m)
	return b
}

// fullSnapshot encodes a mkt message with all symbols (for newly registered clients).
// It does not touch the changed-snapshot baseline.
func (e *engine) fullSnapshot() []byte {
	m := mktMsg{T: "mkt", TS: e.nowMicro(), S: make([][3]int64, numSyms)}
	for s := 0; s < numSyms; s++ {
		m.S[s] = [3]int64{int64(s), e.last[s], e.vol[s]}
	}
	b, _ := json.Marshal(&m)
	return b
}

type logOrderLine struct {
	E    string `json:"e"`
	ID   int64  `json:"id"`
	Acct int64  `json:"acct"`
	Sym  int    `json:"sym"`
	Side string `json:"side"`
	Type string `json:"type"`
	Qty  int64  `json:"qty"`
	Px   int64  `json:"px"`
	TS   int64  `json:"ts"`
}

type logTradeLine struct {
	E    string `json:"e"`
	Sym  int    `json:"sym"`
	Px   int64  `json:"px"`
	Qty  int64  `json:"qty"`
	Buy  int64  `json:"buy"`
	Sell int64  `json:"sell"`
	TS   int64  `json:"ts"`
}

type logCancelLine struct {
	E  string `json:"e"`
	ID int64  `json:"id"`
	TS int64  `json:"ts"`
}

func (e *engine) emitLog(v any) {
	b, _ := json.Marshal(v)
	e.sink.logLine(append(b, '\n'))
}

func (e *engine) logOrder(o *order) {
	side, typ := "S", "L"
	if o.buy {
		side = "B"
	}
	if o.market {
		typ = "M"
	}
	e.emitLog(&logOrderLine{E: "o", ID: o.id, Acct: o.acct, Sym: o.sym, Side: side, Type: typ, Qty: o.qty, Px: o.px, TS: o.ts})
}

func (e *engine) logTrade(sym int, px, qty, buy, sell int64) {
	e.emitLog(&logTradeLine{E: "t", Sym: sym, Px: px, Qty: qty, Buy: buy, Sell: sell, TS: e.nowMicro()})
}

func (e *engine) logCancel(id int64) {
	e.emitLog(&logCancelLine{E: "c", ID: id, TS: e.nowMicro()})
}
