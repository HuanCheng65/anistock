package main

import (
	"strings"
	"testing"
)

type recSink struct {
	logs []string
}

func (r *recSink) logLine(l []byte) { r.logs = append(r.logs, string(l)) }

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
