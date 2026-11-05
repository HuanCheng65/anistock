package main

import (
	"encoding/json"
	"fmt"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/gorilla/websocket"
)

// testServer runs the engine loop with a manually driven NPC tick channel.
type testServer struct {
	s    *server
	tick chan time.Time
	http *httptest.Server
}

func newTestServer(t *testing.T, npcRate int, mm bool) *testServer {
	t.Helper()
	lw, err := newLogWriter(filepath.Join(t.TempDir(), "x.log"))
	if err != nil {
		t.Fatal(err)
	}
	s := newServer(npcRate, mm, lw)
	tick := make(chan time.Time)
	s.tickC = tick
	go s.run()
	hs := httptest.NewServer(s.mux())
	t.Cleanup(hs.Close)
	return &testServer{s: s, tick: tick, http: hs}
}

// doTick runs one NPC tick and returns once the engine has finished it.
func (ts *testServer) doTick() {
	ts.tick <- time.Now()
	ts.s.call(request{kind: kStats})
}

// wsConn wraps a client connection with a reader goroutine (gorilla connections
// are unusable after a read deadline expires, so we never set one).
type wsConn struct {
	*websocket.Conn
	frames chan string
}

func (ts *testServer) dial(t *testing.T, sym int, wantWS int64) *wsConn {
	t.Helper()
	url := "ws" + strings.TrimPrefix(ts.http.URL, "http") + fmt.Sprintf("/ws?sym=%d", sym)
	c, _, err := websocket.DefaultDialer.Dial(url, nil)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { c.Close() })
	wc := &wsConn{Conn: c, frames: make(chan string, 1024)}
	go func() {
		for {
			typ, data, err := c.ReadMessage()
			if err != nil {
				close(wc.frames)
				return
			}
			if typ != websocket.TextMessage {
				data = []byte("BAD FRAME TYPE")
			}
			wc.frames <- string(data)
		}
	}()
	for i := 0; ts.s.call(request{kind: kStats}).stats.WS != wantWS; i++ {
		if i > 1000 {
			t.Fatal("client never registered")
		}
		time.Sleep(time.Millisecond)
	}
	return wc
}

func (ts *testServer) place(t *testing.T, r orderReq) orderResp {
	t.Helper()
	rep := ts.s.call(request{kind: kOrder, order: r})
	if rep.err != nil {
		t.Fatalf("place %+v: %v", r, rep.err)
	}
	return rep.order
}

type wsMsg struct {
	T    string     `json:"t"`
	Sym  *int       `json:"sym"`
	Last int64      `json:"last"`
	Vol  int64      `json:"vol"`
	Bids [][2]int64 `json:"bids"`
	Asks [][2]int64 `json:"asks"`
	TS   int64      `json:"ts"`
	S    [][3]int64 `json:"s"`
}

// readFor reads frames until d elapses with no data; returns frames and parsed messages.
func readFor(t *testing.T, c *wsConn, d time.Duration) (frames []string, msgs []wsMsg) {
	t.Helper()
	for {
		var data string
		select {
		case f, ok := <-c.frames:
			if !ok {
				t.Fatal("connection closed")
			}
			data = f
		case <-time.After(d):
			return
		}
		frames = append(frames, data)
		for _, line := range strings.Split(data, "\n") {
			var m wsMsg
			if err := json.Unmarshal([]byte(line), &m); err != nil {
				t.Fatalf("bad NDJSON line %q: %v", line, err)
			}
			msgs = append(msgs, m)
		}
	}
}

func TestConflationAndSnapshot(t *testing.T) {
	ts := newTestServer(t, 0, false)
	c := ts.dial(t, 5, 1) // connection id 0 -> snapshot slot 0

	// Registration: exactly one full snapshot.
	_, msgs := readFor(t, c, 150*time.Millisecond)
	if len(msgs) != 1 || msgs[0].T != "mkt" || len(msgs[0].S) != numSyms {
		t.Fatalf("initial: %+v", msgs)
	}

	// Many orders/trades on sym 5 (subscribed) and a trade on sym 6 (not subscribed) within one tick.
	for i := 0; i < 20; i++ {
		ts.place(t, orderReq{Acct: int64(100 + i), Sym: 5, Side: "S", Type: "L", Qty: 10, Px: 1500 + int64(i)})
		ts.place(t, orderReq{Acct: int64(200 + i), Sym: 5, Side: "B", Type: "L", Qty: 3, Px: 1550})
	}
	ts.place(t, orderReq{Acct: 1, Sym: 6, Side: "S", Type: "L", Qty: 1, Px: 1600})
	ts.place(t, orderReq{Acct: 2, Sym: 6, Side: "B", Type: "L", Qty: 1, Px: 1600})
	ts.doTick() // k=0: build changed snapshot, deliver to slot 0, flush books

	_, msgs = readFor(t, c, 150*time.Millisecond)
	var books, mkts []wsMsg
	for _, m := range msgs {
		switch m.T {
		case "book":
			books = append(books, m)
		case "mkt":
			mkts = append(mkts, m)
		}
	}
	if len(books) != 1 || *books[0].Sym != 5 {
		t.Fatalf("want exactly one conflated book for sym 5, got %+v", books)
	}
	e := ts.s.eng // engine idle between ticks/calls
	if books[0].Last != e.last[5] || books[0].Vol != 60 || len(books[0].Asks) != 5 {
		t.Fatalf("book not latest state: %+v (last=%d)", books[0], e.last[5])
	}
	if len(mkts) != 1 || len(mkts[0].S) != 2 || mkts[0].S[0][0] != 5 || mkts[0].S[1][0] != 6 ||
		mkts[0].S[1][1] != 1600 || mkts[0].S[1][2] != 1 {
		t.Fatalf("snapshot must hold only changed syms 5,6: %+v", mkts)
	}

	// Ticks 1..9: nothing changed and slot != 0 -> nothing at all.
	for i := 1; i < 10; i++ {
		ts.doTick()
	}
	if _, msgs = readFor(t, c, 100*time.Millisecond); len(msgs) != 0 {
		t.Fatalf("expected silence, got %+v", msgs)
	}
	// A cancel marks dirty too. Tick 10: new snapshot (nothing changed -> "s":[]) and one book.
	r := ts.place(t, orderReq{Acct: 300, Sym: 5, Side: "B", Type: "L", Qty: 1, Px: 1400})
	if !ts.s.call(request{kind: kCancel, acct: 300, id: r.ID}).ok {
		t.Fatal("cancel failed")
	}
	ts.doTick()
	frames, msgs := readFor(t, c, 150*time.Millisecond)
	joined := strings.Join(frames, "\n")
	if !strings.Contains(joined, `{"t":"mkt","ts":`) || !strings.Contains(joined, `"s":[]}`) {
		t.Fatalf("want empty snapshot, got %q", joined)
	}
	if len(msgs) != 2 {
		t.Fatalf("want mkt+book, got %q", joined)
	}
}

func TestSnapshotSlots(t *testing.T) {
	ts := newTestServer(t, 0, false)
	var conns []*wsConn
	for i := 0; i < 3; i++ { // ids 0,1,2
		conns = append(conns, ts.dial(t, 0, int64(i+1)))
	}
	for _, c := range conns {
		readFor(t, c, 100*time.Millisecond) // initial full snapshot
	}
	got := make([]int, 3)
	for k := 0; k < 3; k++ {
		ts.doTick()
		for i, c := range conns {
			_, msgs := readFor(t, c, 60*time.Millisecond)
			if len(msgs) > 0 {
				if len(msgs) != 1 || msgs[0].T != "mkt" || got[i] != 0 {
					t.Fatalf("client %d tick %d: %+v", i, k, msgs)
				}
				got[i] = k + 1
			}
		}
	}
	if got[0] != 1 || got[1] != 2 || got[2] != 3 {
		t.Fatalf("client i must get the snapshot on tick i: %v", got)
	}
}

func TestDrainBatch(t *testing.T) {
	ch := make(chan []byte, wsQueueCap)
	ch <- []byte(`{"b":2}`)
	ch <- []byte(`{"c":3}`)
	out, open := drainBatch(nil, []byte(`{"a":1}`), ch)
	if string(out) != "{\"a\":1}\n{\"b\":2}\n{\"c\":3}" || !open || len(ch) != 0 {
		t.Fatalf("got %q open=%v", out, open)
	}
	for i := 0; i < wsQueueCap; i++ {
		ch <- []byte("x")
	}
	out, _ = drainBatch(out[:0], []byte("x"), ch)
	if n := strings.Count(string(out), "\n") + 1; n != wsQueueCap || len(ch) != 1 {
		t.Fatalf("batch of %d, %d left", n, len(ch))
	}
	close(ch)
	if _, open = drainBatch(nil, []byte("y"), ch); open {
		t.Fatal("closed channel not reported")
	}
}

func TestSlowClientDropped(t *testing.T) {
	ts := newTestServer(t, 0, false)
	ts.dial(t, 5, 1) // never read from it
	for i := 0; i < 400000; i++ {
		side := "B"
		if i%2 == 1 {
			side = "S"
		}
		ts.place(t, orderReq{Acct: int64(i % 5000), Sym: 5, Side: side, Type: "L", Qty: 1, Px: 1400 + int64(i%200)})
		ts.tick <- time.Now()
		if i%1000 == 999 {
			if st := ts.s.call(request{kind: kStats}).stats; st.Dropped == 1 {
				if st.WS != 0 {
					t.Fatalf("ws=%d after drop", st.WS)
				}
				t.Logf("dropped after %d ticks", i+1)
				return
			}
		}
	}
	t.Fatal("slow client never dropped")
}
