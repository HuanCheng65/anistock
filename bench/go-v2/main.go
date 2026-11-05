package main

import (
	"encoding/json"
	"log"
	"net"
	"net/http"
	"os"
	"strconv"
	"sync"
	"syscall"
	"time"

	"github.com/gorilla/websocket"
)

const wsQueueCap = 256

// ---- event log writer ----

// logWriter is an unbounded queue (mutex-guarded byte buffer the writer swaps out)
// so the engine never blocks on disk. Every 50 ms the writer appends what has
// accumulated and fdatasyncs if anything was written (group commit).
type logWriter struct {
	mu   sync.Mutex
	buf  []byte
	f    *os.File
	done chan struct{}
}

func newLogWriter(path string) (*logWriter, error) {
	f, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY|os.O_TRUNC, 0o644)
	if err != nil {
		return nil, err
	}
	lw := &logWriter{f: f, done: make(chan struct{})}
	go lw.run()
	return lw, nil
}

func (lw *logWriter) append(line []byte) {
	lw.mu.Lock()
	lw.buf = append(lw.buf, line...)
	lw.mu.Unlock()
}

func (lw *logWriter) run() {
	t := time.NewTicker(50 * time.Millisecond)
	defer t.Stop()
	var spare []byte
	for range t.C {
		lw.mu.Lock()
		out := lw.buf
		lw.buf = spare[:0]
		lw.mu.Unlock()
		if len(out) > 0 {
			if _, err := lw.f.Write(out); err != nil {
				log.Printf("log write: %v", err)
			}
			if err := syscall.Fdatasync(int(lw.f.Fd())); err != nil {
				log.Printf("fdatasync: %v", err)
			}
		}
		spare = out
	}
}

// ---- websocket clients ----

type wsClient struct {
	conn *websocket.Conn
	id   int64 // connection id, assigned by the engine at registration
	sym  int
	ch   chan []byte
}

// drainBatch appends first plus every message already queued in ch (at most
// wsQueueCap in total), joined with '\n', to buf. It never waits for more.
// open is false if ch was found closed.
func drainBatch(buf, first []byte, ch chan []byte) (out []byte, open bool) {
	buf = append(buf, first...)
	for n := 1; n < wsQueueCap; n++ {
		select {
		case m, ok := <-ch:
			if !ok {
				return buf, false
			}
			buf = append(buf, '\n')
			buf = append(buf, m...)
		default:
			return buf, true
		}
	}
	return buf, true
}

// writer sends everything queued as one NDJSON text frame per drain
// (gorilla's server-side WriteMessage emits a single frame with one writev).
func (c *wsClient) writer() {
	defer c.conn.Close()
	var buf []byte
	broken := false
	for first := range c.ch {
		if broken {
			continue // connection broken; drain until the engine closes ch
		}
		frame, open := drainBatch(buf[:0], first, c.ch)
		if err := c.conn.WriteMessage(websocket.TextMessage, frame); err != nil {
			broken = true
			c.conn.Close()
		}
		if cap(frame) <= 16<<10 {
			buf = frame // reuse modest buffers; let big ones go back to the GC
		} else {
			buf = nil
		}
		if !open {
			return
		}
	}
}

// ---- engine loop ----

type reqKind int

const (
	kOrder reqKind = iota
	kCancel
	kStats
	kWSAdd
	kWSDel
)

type request struct {
	kind   reqKind
	order  orderReq
	acct   int64
	id     int64
	client *wsClient
	reply  chan reply
}

type reply struct {
	order orderResp
	err   error
	ok    bool
	stats stats
}

type server struct {
	eng     *engine
	reqs    chan request
	lw      *logWriter
	subs    [numSyms]map[*wsClient]struct{}
	slots   [10]map[*wsClient]struct{} // clients by connection id % 10
	clients map[*wsClient]struct{}
	nextCID int64
	npcRate int
	mm      bool

	tickC <-chan time.Time // NPC tick source (100 ms ticker; tests may inject their own)
	k     int              // NPC tick counter
	snap  []byte           // current mkt snapshot being delivered over 10 ticks
}

func (s *server) logLine(line []byte) { s.lw.append(line) }

func (s *server) enqueue(c *wsClient, msg []byte) {
	select {
	case c.ch <- msg:
	default:
		if s.removeClient(c) {
			s.eng.st.Dropped++
			c.conn.Close()
		}
	}
}

func (s *server) removeClient(c *wsClient) bool {
	if _, ok := s.clients[c]; !ok {
		return false
	}
	delete(s.clients, c)
	delete(s.subs[c.sym], c)
	delete(s.slots[c.id%10], c)
	close(c.ch)
	return true
}

func (s *server) run() {
	if s.tickC == nil {
		t := time.NewTicker(100 * time.Millisecond)
		defer t.Stop()
		s.tickC = t.C
	}
	for {
		select {
		case r := <-s.reqs:
			s.handle(r)
		case <-s.tickC:
			s.tick()
		}
	}
}

// tick is one 100 ms NPC tick. Order: NPC expiry + NPC orders, staggered market
// makers, (every 10th tick) build the changed-symbols snapshot, deliver the
// snapshot to this tick's connection-id slot, then flush conflated book events.
func (s *server) tick() {
	k := s.k
	s.k++
	s.eng.npcTick(s.npcRate / 10)
	if s.mm {
		s.eng.mmTick(k)
	}
	if k%10 == 0 {
		s.snap = s.eng.changedSnapshot()
	}
	for c := range s.slots[k%10] {
		s.enqueue(c, s.snap)
	}
	s.eng.flushDirty(func(sym int) {
		if len(s.subs[sym]) == 0 {
			return
		}
		msg := s.eng.bookMessage(sym)
		for c := range s.subs[sym] {
			s.enqueue(c, msg)
		}
	})
}

func (s *server) handle(r request) {
	switch r.kind {
	case kOrder:
		resp, err := s.eng.placeOrder(&r.order)
		if err == nil {
			s.eng.st.Orders++
		}
		r.reply <- reply{order: resp, err: err}
	case kCancel:
		r.reply <- reply{ok: s.eng.cancel(r.acct, r.id)}
	case kStats:
		st := s.eng.st
		st.WS = int64(len(s.clients))
		r.reply <- reply{stats: st}
	case kWSAdd:
		c := r.client
		c.id = s.nextCID
		s.nextCID++
		s.clients[c] = struct{}{}
		s.subs[c.sym][c] = struct{}{}
		s.slots[c.id%10][c] = struct{}{}
		s.enqueue(c, s.eng.fullSnapshot())
	case kWSDel:
		s.removeClient(r.client)
	}
}

func (s *server) call(r request) reply {
	r.reply = make(chan reply, 1)
	s.reqs <- r
	return <-r.reply
}

// ---- HTTP ----

func writeJSON(w http.ResponseWriter, code int, v any) {
	b, _ := json.Marshal(v)
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Content-Length", strconv.Itoa(len(b)))
	w.WriteHeader(code)
	w.Write(b)
}

type errBody struct {
	Error string `json:"error"`
}

func (s *server) handleOrder(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		writeJSON(w, http.StatusMethodNotAllowed, errBody{"method"})
		return
	}
	var req orderReq
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, errBody{"bad json"})
		return
	}
	rep := s.call(request{kind: kOrder, order: req})
	if rep.err != nil {
		writeJSON(w, http.StatusBadRequest, errBody{rep.err.Error()})
		return
	}
	writeJSON(w, http.StatusOK, &rep.order)
}

type cancelReq struct {
	Acct int64 `json:"acct"`
	ID   int64 `json:"id"`
}

type cancelResp struct {
	OK bool `json:"ok"`
}

func (s *server) handleCancel(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		writeJSON(w, http.StatusMethodNotAllowed, errBody{"method"})
		return
	}
	var req cancelReq
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, errBody{"bad json"})
		return
	}
	rep := s.call(request{kind: kCancel, acct: req.Acct, id: req.ID})
	writeJSON(w, http.StatusOK, cancelResp{rep.ok})
}

func (s *server) handleStats(w http.ResponseWriter, r *http.Request) {
	rep := s.call(request{kind: kStats})
	writeJSON(w, http.StatusOK, &rep.stats)
}

// Small per-connection buffers (SPEC-v2 §4): clients never send, so a tiny read
// buffer suffices; the write buffer comes from a pool and is held only while a
// frame is being written, so idle connections hold none.
var upgrader = websocket.Upgrader{
	ReadBufferSize:  256,
	WriteBufferSize: 4096,
	WriteBufferPool: &sync.Pool{},
	CheckOrigin:     func(*http.Request) bool { return true },
}

func (s *server) handleWS(w http.ResponseWriter, r *http.Request) {
	sym, err := strconv.Atoi(r.URL.Query().Get("sym"))
	if err != nil || sym < 0 || sym >= numSyms {
		writeJSON(w, http.StatusBadRequest, errBody{"bad sym"})
		return
	}
	conn, err := upgrader.Upgrade(w, r, nil)
	if err != nil {
		return
	}
	c := &wsClient{conn: conn, sym: sym, ch: make(chan []byte, wsQueueCap)}
	go c.writer()
	s.reqs <- request{kind: kWSAdd, client: c}
	// Reader: discard everything; on error/close, unregister.
	go func() {
		for {
			if _, _, err := conn.NextReader(); err != nil {
				break
			}
		}
		s.reqs <- request{kind: kWSDel, client: c}
	}()
}

func newServer(npcRate int, mm bool, lw *logWriter) *server {
	s := &server{
		reqs:    make(chan request, 4096),
		lw:      lw,
		clients: make(map[*wsClient]struct{}),
		npcRate: npcRate,
		mm:      mm,
	}
	for i := range s.subs {
		s.subs[i] = make(map[*wsClient]struct{})
	}
	for i := range s.slots {
		s.slots[i] = make(map[*wsClient]struct{})
	}
	s.eng = newEngine(s)
	return s
}

func (s *server) mux() *http.ServeMux {
	mux := http.NewServeMux()
	mux.HandleFunc("/health", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/plain")
		w.Write([]byte("ok"))
	})
	mux.HandleFunc("/order", s.handleOrder)
	mux.HandleFunc("/cancel", s.handleCancel)
	mux.HandleFunc("/stats", s.handleStats)
	mux.HandleFunc("/ws", s.handleWS)
	return mux
}

func envInt(name string, def int) int {
	if v := os.Getenv(name); v != "" {
		if n, err := strconv.Atoi(v); err == nil {
			return n
		}
		log.Fatalf("bad %s=%q", name, v)
	}
	return def
}

func main() {
	port := envInt("BENCH_PORT", 18700)
	npcRate := envInt("BENCH_NPC_RATE", 300)
	mm := envInt("BENCH_MM", 1) == 1
	logPath := os.Getenv("BENCH_LOG")
	if logPath == "" {
		logPath = "bench-go.log"
	}
	lw, err := newLogWriter(logPath)
	if err != nil {
		log.Fatal(err)
	}
	s := newServer(npcRate, mm, lw)
	go s.run()

	addr := net.JoinHostPort("127.0.0.1", strconv.Itoa(port))
	log.Printf("anistock-bench go: listening on %s (npc=%d/s mm=%v log=%s)", addr, npcRate, mm, logPath)
	log.Fatal(http.ListenAndServe(addr, s.mux()))
}
