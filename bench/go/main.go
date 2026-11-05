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
	sym  int
	ch   chan []byte
}

func (c *wsClient) writer() {
	ok := true
	for msg := range c.ch {
		if !ok {
			continue // connection broken; drain until the engine closes ch
		}
		if err := c.conn.WriteMessage(websocket.TextMessage, msg); err != nil {
			ok = false
			c.conn.Close()
		}
	}
	c.conn.Close()
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
	clients map[*wsClient]struct{}
	npcRate int
	mm      bool
}

// bookEvent implements sink: one shared encoded message to every subscriber of sym.
func (s *server) bookEvent(sym int, msg []byte) {
	for c := range s.subs[sym] {
		s.enqueue(c, msg)
	}
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
	close(c.ch)
	return true
}

func (s *server) run() {
	npc := time.NewTicker(100 * time.Millisecond)
	sec := time.NewTicker(1000 * time.Millisecond)
	defer npc.Stop()
	defer sec.Stop()
	perTick := s.npcRate / 10
	k := 0 // NPC tick counter
	for {
		select {
		case r := <-s.reqs:
			s.handle(r)
		case <-npc.C:
			// Order within a tick: NPC expiry, NPC orders, then staggered market makers.
			s.eng.npcTick(perTick)
			if s.mm {
				s.eng.mmTick(k)
			}
			k++
		case <-sec.C:
			msg := s.eng.mktSnapshot()
			for c := range s.clients {
				s.enqueue(c, msg)
			}
		}
	}
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
		s.clients[r.client] = struct{}{}
		s.subs[r.client.sym][r.client] = struct{}{}
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

var upgrader = websocket.Upgrader{
	ReadBufferSize:  1024,
	WriteBufferSize: 4096,
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
