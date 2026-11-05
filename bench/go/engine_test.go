package main

import (
	"encoding/json"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/gorilla/websocket"
)

type recSink struct {
	books [][]byte
	logs  []string
}

func (r *recSink) bookEvent(sym int, msg []byte) { r.books = append(r.books, msg) }
func (r *recSink) logLine(l []byte)               { r.logs = append(r.logs, string(l)) }

func mustPlace(t *testing.T, e *engine, r orderReq) orderResp {
	t.Helper()
	resp, err := e.placeOrder(&r)
	if err != nil {
		t.Fatalf("place %+v: %v", r, err)
	}
	return resp
}

func TestPriceTimePriority(t *testing.T) {
	e := newEngine(&recSink{})
	// sym 0: last=1000
	a1 := mustPlace(t, e, orderReq{Acct: 1, Sym: 0, Side: "S", Type: "L", Qty: 10, Px: 1010})
	a2 := mustPlace(t, e, orderReq{Acct: 2, Sym: 0, Side: "S", Type: "L", Qty: 10, Px: 1005})
	a3 := mustPlace(t, e, orderReq{Acct: 3, Sym: 0, Side: "S", Type: "L", Qty: 10, Px: 1005})
	// buy 15 @1010: should hit a2 (10 @1005) then a3 (5 @1005), not a1.
	r := mustPlace(t, e, orderReq{Acct: 4, Sym: 0, Side: "B", Type: "L", Qty: 15, Px: 1010})
	if r.Status != "filled" || r.Filled != 15 {
		t.Fatalf("got %+v", r)
	}
	if e.open[a2.ID] != nil || e.open[a3.ID].filled != 5 || e.open[a1.ID].filled != 0 {
		t.Fatal("wrong priority")
	}
	if e.last[0] != 1005 || e.vol[0] != 15 {
		t.Fatalf("last=%d vol=%d", e.last[0], e.vol[0])
	}
	// Buyer paid resting price 1005; frozen reserve at 1010 fully released.
	if a := e.accounts[4]; a.cash != startCash-15*1005 || a.cashFrozen != 0 {
		t.Fatalf("buyer acct %+v", a)
	}
	asks := e.books[0].asks.depth(5)
	if len(asks) != 2 || asks[0] != [2]int64{1005, 5} || asks[1] != [2]int64{1010, 10} {
		t.Fatalf("asks %v", asks)
	}
	if h := e.holding(2, 0); h.shares != startShares-10 || h.sharesFrozen != 0 {
		t.Fatalf("seller holding %+v", h)
	}
}

func TestSelfTradeCancelsIncoming(t *testing.T) {
	e := newEngine(&recSink{})
	mustPlace(t, e, orderReq{Acct: 2, Sym: 0, Side: "S", Type: "L", Qty: 5, Px: 1000})
	own := mustPlace(t, e, orderReq{Acct: 1, Sym: 0, Side: "S", Type: "L", Qty: 5, Px: 1001})
	mustPlace(t, e, orderReq{Acct: 3, Sym: 0, Side: "S", Type: "L", Qty: 5, Px: 1002})
	// acct 1 buys 20 @1002: fills 5 from acct 2, then hits own ask -> stop, cancel remainder.
	r := mustPlace(t, e, orderReq{Acct: 1, Sym: 0, Side: "B", Type: "L", Qty: 20, Px: 1002})
	if r.Status != "cancelled" || r.Filled != 5 {
		t.Fatalf("got %+v", r)
	}
	if e.open[own.ID] == nil || e.open[r.ID] != nil {
		t.Fatal("resting own order must stay; incoming must not rest")
	}
	if a := e.accounts[1]; a.cashFrozen != 0 || a.cash != startCash-5*1000 {
		t.Fatalf("acct1 %+v", a)
	}
	if bids := e.books[0].bids.depth(5); len(bids) != 0 {
		t.Fatalf("bids %v", bids)
	}
}

func TestFrozenReleaseAndCancel(t *testing.T) {
	s := &recSink{}
	e := newEngine(s)
	b := mustPlace(t, e, orderReq{Acct: 7, Sym: 1, Side: "B", Type: "L", Qty: 100, Px: 1100})
	if e.accounts[7].cashFrozen != 110000 {
		t.Fatalf("frozen %d", e.accounts[7].cashFrozen)
	}
	mustPlace(t, e, orderReq{Acct: 8, Sym: 1, Side: "S", Type: "L", Qty: 30, Px: 1050})
	// Seller's limit 1050 crosses resting bid 1100 at price 1100.
	if e.accounts[7].cashFrozen != 70*1100 || e.last[1] != 1100 {
		t.Fatalf("frozen %d last %d", e.accounts[7].cashFrozen, e.last[1])
	}
	if e.cancel(8, b.ID) {
		t.Fatal("cancel by wrong acct succeeded")
	}
	if !e.cancel(7, b.ID) || e.accounts[7].cashFrozen != 0 {
		t.Fatalf("cancel; frozen=%d", e.accounts[7].cashFrozen)
	}
	if e.cancel(7, b.ID) {
		t.Fatal("double cancel")
	}
	// Market buy against empty book: accepted, cancelled, reserve released.
	m := mustPlace(t, e, orderReq{Acct: 9, Sym: 1, Side: "B", Type: "M", Qty: 10})
	if m.Status != "cancelled" || m.Filled != 0 || e.accounts[9].cashFrozen != 0 {
		t.Fatalf("market %+v frozen %d", m, e.accounts[9].cashFrozen)
	}
	// Market sell partially filled; shares_frozen released.
	mustPlace(t, e, orderReq{Acct: 10, Sym: 1, Side: "B", Type: "L", Qty: 4, Px: 1100})
	ms := mustPlace(t, e, orderReq{Acct: 11, Sym: 1, Side: "S", Type: "M", Qty: 10})
	if ms.Status != "cancelled" || ms.Filled != 4 {
		t.Fatalf("market sell %+v", ms)
	}
	if h := e.holding(11, 1); h.sharesFrozen != 0 || h.shares != startShares-4 {
		t.Fatalf("holding %+v", h)
	}
	// Band check.
	if _, err := e.placeOrder(&orderReq{Acct: 1, Sym: 1, Side: "B", Type: "L", Qty: 1, Px: 1100*2 + 1}); err == nil {
		t.Fatal("out of band accepted")
	}
	var nc int
	for _, l := range s.logs {
		if strings.HasPrefix(l, `{"e":"c"`) {
			nc++
		}
	}
	if nc != 1 {
		t.Fatalf("c lines %d", nc)
	}
}

func TestWebSocketMessages(t *testing.T) {
	dir := t.TempDir()
	lw, err := newLogWriter(filepath.Join(dir, "x.log"))
	if err != nil {
		t.Fatal(err)
	}
	s := newServer(100, true, lw)
	go s.run()
	ts := httptest.NewServer(s.mux())
	defer ts.Close()
	url := "ws" + strings.TrimPrefix(ts.URL, "http") + "/ws?sym=3"
	c, _, err := websocket.DefaultDialer.Dial(url, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	seen := map[string]bool{}
	c.SetReadDeadline(time.Now().Add(5 * time.Second))
	for !(seen["book"] && seen["mkt"]) {
		_, data, err := c.ReadMessage()
		if err != nil {
			t.Fatal(err)
		}
		var m map[string]json.RawMessage
		if err := json.Unmarshal(data, &m); err != nil {
			t.Fatal(err)
		}
		var typ string
		json.Unmarshal(m["t"], &typ)
		keys := make([]string, 0, len(m))
		for k := range m {
			keys = append(keys, k)
		}
		switch typ {
		case "book":
			for _, k := range []string{"sym", "last", "vol", "bids", "asks", "ts"} {
				if _, ok := m[k]; !ok {
					t.Fatalf("book missing %s: %s", k, data)
				}
			}
			if string(m["sym"]) != "3" || len(m) != 7 {
				t.Fatalf("book %s", data)
			}
		case "mkt":
			var arr [][3]int64
			if err := json.Unmarshal(m["s"], &arr); err != nil || len(arr) != numSyms || len(m) != 3 {
				t.Fatalf("mkt %s", data)
			}
		default:
			t.Fatalf("unknown %s", data)
		}
		if !seen[typ] {
			t.Logf("%s keys=%v sample=%.200s", typ, keys, data)
		}
		seen[typ] = true
	}
	_ = os.Remove
}

func TestSlowClientDropped(t *testing.T) {
	lw, err := newLogWriter(filepath.Join(t.TempDir(), "x.log"))
	if err != nil {
		t.Fatal(err)
	}
	s := newServer(0, false, lw)
	go s.run()
	ts := httptest.NewServer(s.mux())
	defer ts.Close()
	url := "ws" + strings.TrimPrefix(ts.URL, "http") + "/ws?sym=5"
	c, _, err := websocket.DefaultDialer.Dial(url, nil) // never read from it
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	for i := 0; i < 200000; i++ {
		side := "B"
		if i%2 == 1 {
			side = "S"
		}
		s.call(request{kind: kOrder, order: orderReq{Acct: int64(i % 5000), Sym: 5, Side: side, Type: "L", Qty: 1, Px: 1400 + int64(i%200)}})
		if i%1000 == 999 {
			if st := s.call(request{kind: kStats}).stats; st.Dropped == 1 {
				if st.WS != 0 {
					t.Fatalf("ws=%d after drop", st.WS)
				}
				t.Logf("dropped after %d orders", i+1)
				return
			}
		}
	}
	t.Fatal("slow client never dropped")
}
