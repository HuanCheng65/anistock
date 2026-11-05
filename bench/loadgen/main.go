// Load generator for bench/SPEC.md servers. One binary drives every implementation with identical traffic.
//
// Open loop: orders are scheduled at a fixed rate whatever the server does, and latency is measured from the
// scheduled send time, so a stalled server shows up as latency instead of silently lowering the load.
package main

import (
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"math"
	"math/rand"
	"net"
	"net/http"
	"os"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/gorilla/websocket"
)

const nsym = 141

// hist: 10 µs buckets up to 20 s, atomic counters.
type hist struct{ b []uint64 }

const bucketUS = 10

func newHist() *hist { return &hist{b: make([]uint64, 2_000_000)} }
func (h *hist) add(us int64) {
	i := us / bucketUS
	if i < 0 {
		i = 0
	}
	if i >= int64(len(h.b)) {
		i = int64(len(h.b)) - 1
	}
	atomic.AddUint64(&h.b[i], 1)
}
func (h *hist) count() uint64 {
	var n uint64
	for i := range h.b {
		n += atomic.LoadUint64(&h.b[i])
	}
	return n
}

// q returns the quantile in milliseconds (upper edge of the bucket).
func (h *hist) q(p float64) float64 {
	n := h.count()
	if n == 0 {
		return math.NaN()
	}
	want := uint64(math.Ceil(p * float64(n)))
	if want == 0 {
		want = 1
	}
	var c uint64
	for i := range h.b {
		c += atomic.LoadUint64(&h.b[i])
		if c >= want {
			return float64((i+1)*bucketUS) / 1000
		}
	}
	return math.NaN()
}

var measuring atomic.Bool

func nowUS() int64 { return time.Now().UnixMicro() }

// tsOf pulls the trailing "ts":<int> out of a message without a full JSON decode (keeps the generator cheap).
func tsOf(msg []byte) (int64, bool) {
	i := bytes.LastIndex(msg, []byte(`"ts":`))
	if i < 0 {
		return 0, false
	}
	j := i + 5
	k := j
	for k < len(msg) && msg[k] >= '0' && msg[k] <= '9' {
		k++
	}
	v, err := strconv.ParseInt(string(msg[j:k]), 10, 64)
	return v, err == nil
}

type wsStats struct {
	lag                         *hist
	msgs, bytes, closed, failed atomic.Uint64
	book, mkt, frames           atomic.Uint64
}

func runWS(base string, n int, st *wsStats, stop <-chan struct{}, wg *sync.WaitGroup) {
	u := strings.Replace(base, "http://", "ws://", 1)
	dialer := websocket.Dialer{HandshakeTimeout: 10 * time.Second, Proxy: nil}
	sem := make(chan struct{}, 64)
	for i := 0; i < n; i++ {
		wg.Add(1)
		sem <- struct{}{}
		go func(i int) {
			defer wg.Done()
			c, _, err := dialer.Dial(fmt.Sprintf("%s/ws?sym=%d", u, i%nsym), nil)
			<-sem
			if err != nil {
				st.failed.Add(1)
				return
			}
			go func() { <-stop; c.Close() }()
			for {
				_, msg, err := c.ReadMessage()
				if err != nil {
					select {
					case <-stop:
					default:
						st.closed.Add(1) // server dropped us (or crashed) before the end
					}
					return
				}
				if !measuring.Load() {
					continue
				}
				st.frames.Add(1)
				st.bytes.Add(uint64(len(msg)))
				// a frame holds one message (v1) or several separated by '\n' (v2 batched writes)
				for _, part := range bytes.Split(msg, []byte{'\n'}) {
					if len(part) == 0 {
						continue
					}
					st.msgs.Add(1)
					// lag is measured on book messages only: v2 spreads each market snapshot over a second
					// on purpose, so its age says nothing about delivery speed
					if bytes.Contains(part[:min(len(part), 16)], []byte(`"book"`)) {
						st.book.Add(1)
						if ts, ok := tsOf(part); ok {
							st.lag.add(nowUS() - ts)
						}
					} else {
						st.mkt.Add(1)
					}
				}
			}
		}(i)
	}
}

type orderStats struct {
	lat                        *hist
	sent, ok, rejected, failed atomic.Uint64
	cancels, cancelOK          atomic.Uint64
}

type openIDs struct {
	mu  sync.Mutex
	ids map[int][]int64 // acct -> recent open order ids
}

func (o *openIDs) push(acct int, id int64) {
	o.mu.Lock()
	l := append(o.ids[acct], id)
	if len(l) > 8 {
		l = l[len(l)-8:]
	}
	o.ids[acct] = l
	o.mu.Unlock()
}
func (o *openIDs) pop(acct int) (int64, bool) {
	o.mu.Lock()
	defer o.mu.Unlock()
	l := o.ids[acct]
	if len(l) == 0 {
		return 0, false
	}
	id := l[len(l)-1]
	o.ids[acct] = l[:len(l)-1]
	return id, true
}

func main() {
	base := flag.String("url", "http://127.0.0.1:18700", "server")
	rate := flag.Float64("rate", 200, "player orders per second (incl. cancels)")
	nws := flag.Int("ws", 500, "websocket clients")
	dur := flag.Duration("dur", 30*time.Second, "measurement window")
	warm := flag.Duration("warmup", 5*time.Second, "warmup before measuring")
	pid := flag.Int("pid", 0, "server pid for CPU / RSS sampling")
	label := flag.String("label", "", "label copied into the result")
	flag.Parse()

	tr := &http.Transport{Proxy: nil, MaxIdleConns: 4096, MaxIdleConnsPerHost: 4096, MaxConnsPerHost: 1024,
		IdleConnTimeout: 90 * time.Second, DialContext: (&net.Dialer{Timeout: 5 * time.Second}).DialContext}
	client := &http.Client{Transport: tr, Timeout: 30 * time.Second}

	stop := make(chan struct{})
	var wg sync.WaitGroup
	ws := &wsStats{lag: newHist()}
	t0 := time.Now()
	runWS(*base, *nws, ws, stop, &wg)
	connectSecs := time.Since(t0).Seconds()

	os_ := &orderStats{lat: newHist()}
	open := &openIDs{ids: map[int][]int64{}}
	var inflight sync.WaitGroup

	post := func(path string, body []byte) (*http.Response, error) {
		req, _ := http.NewRequest("POST", *base+path, bytes.NewReader(body))
		req.Header.Set("Content-Type", "application/json")
		return client.Do(req)
	}

	doOne := func(sched time.Time, r *rand.Rand) {
		defer inflight.Done()
		acct := r.Intn(10000)
		rec := measuring.Load()
		p := r.Float64()
		var resp *http.Response
		var err error
		isCancel := false
		if p < 0.15 {
			if id, ok := open.pop(acct); ok {
				isCancel = true
				resp, err = post("/cancel", []byte(fmt.Sprintf(`{"acct":%d,"id":%d}`, acct, id)))
			}
		}
		if !isCancel {
			sym := r.Intn(nsym)
			side := "B"
			if r.Intn(2) == 0 {
				side = "S"
			}
			var body string
			if p < 0.75 {
				px := int(math.Round(float64(1000+100*sym) * (1 + (r.Float64()*2-1)*0.03)))
				body = fmt.Sprintf(`{"acct":%d,"sym":%d,"side":"%s","type":"L","qty":%d,"px":%d}`, acct, sym, side, 1+r.Intn(100), px)
			} else {
				body = fmt.Sprintf(`{"acct":%d,"sym":%d,"side":"%s","type":"M","qty":%d}`, acct, sym, side, 1+r.Intn(50))
			}
			resp, err = post("/order", []byte(body))
		}
		if err != nil {
			if rec {
				os_.failed.Add(1)
			}
			return
		}
		b, _ := io.ReadAll(resp.Body)
		resp.Body.Close()
		lat := time.Since(sched).Microseconds()
		if !rec {
			return
		}
		os_.sent.Add(1)
		os_.lat.add(lat)
		if resp.StatusCode == 400 {
			os_.rejected.Add(1)
			return
		}
		if resp.StatusCode != 200 {
			os_.failed.Add(1)
			return
		}
		os_.ok.Add(1)
		if isCancel {
			os_.cancels.Add(1)
			if bytes.Contains(b, []byte("true")) {
				os_.cancelOK.Add(1)
			}
			return
		}
		var or struct {
			ID     int64  `json:"id"`
			Status string `json:"status"`
		}
		if json.Unmarshal(b, &or) == nil && or.Status == "open" {
			open.push(acct, or.ID)
		}
	}

	// scheduler
	schedDone := make(chan struct{})
	var behind atomic.Int64 // max scheduler lag (µs): the generator itself falling behind
	go func() {
		defer close(schedDone)
		interval := time.Duration(float64(time.Second) / *rate)
		start := time.Now()
		end := start.Add(*warm + *dur)
		r := rand.New(rand.NewSource(1))
		for i := int64(0); ; i++ {
			sched := start.Add(time.Duration(i) * interval)
			if sched.After(end) {
				return
			}
			if d := time.Until(sched); d > 0 {
				time.Sleep(d)
			} else if -d.Microseconds() > behind.Load() {
				behind.Store(-d.Microseconds())
			}
			inflight.Add(1)
			go doOne(sched, rand.New(rand.NewSource(r.Int63())))
		}
	}()

	getStats := func() map[string]float64 {
		m := map[string]float64{}
		resp, err := client.Get(*base + "/stats")
		if err != nil {
			return m
		}
		defer resp.Body.Close()
		json.NewDecoder(resp.Body).Decode(&m)
		return m
	}
	cpuTicks := func() float64 {
		b, err := os.ReadFile(fmt.Sprintf("/proc/%d/stat", *pid))
		if err != nil {
			return math.NaN()
		}
		f := strings.Fields(string(b[bytes.LastIndexByte(b, ')')+2:]))
		ut, _ := strconv.ParseFloat(f[11], 64)
		st, _ := strconv.ParseFloat(f[12], 64)
		return ut + st
	}
	rssMB := func() float64 {
		b, err := os.ReadFile(fmt.Sprintf("/proc/%d/status", *pid))
		if err != nil {
			return math.NaN()
		}
		for _, l := range strings.Split(string(b), "\n") {
			if strings.HasPrefix(l, "VmRSS:") {
				kb, _ := strconv.ParseFloat(strings.Fields(l)[1], 64)
				return kb / 1024
			}
		}
		return math.NaN()
	}

	time.Sleep(*warm)
	s0, c0, w0 := getStats(), cpuTicks(), time.Now()
	measuring.Store(true)
	time.Sleep(*dur)
	measuring.Store(false)
	s1, c1, secs := getStats(), cpuTicks(), time.Since(w0).Seconds()
	rss := rssMB()
	<-schedDone
	waitCh := make(chan struct{})
	go func() { inflight.Wait(); close(waitCh) }()
	select {
	case <-waitCh:
	case <-time.After(30 * time.Second):
	}
	close(stop)

	res := map[string]any{
		"label":           *label,
		"target_rate":     *rate,
		"ws_clients":      *nws,
		"ws_connect_s":    round(connectSecs),
		"ws_failed":       ws.failed.Load(),
		"ws_dropped":      ws.closed.Load(),
		"orders_per_s":    round(float64(os_.sent.Load()) / secs),
		"order_ok":        os_.ok.Load(),
		"order_rejected":  os_.rejected.Load(),
		"order_failed":    os_.failed.Load(),
		"cancels":         os_.cancels.Load(),
		"rtt_p50_ms":      round(os_.lat.q(0.50)),
		"rtt_p90_ms":      round(os_.lat.q(0.90)),
		"rtt_p99_ms":      round(os_.lat.q(0.99)),
		"rtt_p999_ms":     round(os_.lat.q(0.999)),
		"rtt_max_ms":      round(os_.lat.q(1.0)),
		"ws_msgs_per_s":   round(float64(ws.msgs.Load()) / secs),
		"ws_frames_per_s": round(float64(ws.frames.Load()) / secs),
		"ws_mbit_per_s":   round(float64(ws.bytes.Load()) * 8 / 1e6 / secs),
		"ws_lag_p50_ms":   round(ws.lag.q(0.50)),
		"ws_lag_p99_ms":   round(ws.lag.q(0.99)),
		"ws_lag_max_ms":   round(ws.lag.q(1.0)),
		"npc_per_s":       round((s1["npc"] - s0["npc"]) / secs),
		"trades_per_s":    round((s1["trades"] - s0["trades"]) / secs),
		"server_dropped":  s1["dropped"],
		"server_cpu":      round((c1 - c0) / 100 / secs), // cores used (CLK_TCK = 100)
		"server_rss_mb":   round(rss),
		"gen_behind_ms":   round(float64(behind.Load()) / 1000),
	}
	out, _ := json.Marshal(res)
	fmt.Println(string(out))
}

func round(x float64) float64 {
	if math.IsNaN(x) || math.IsInf(x, 0) {
		return -1
	}
	return math.Round(x*100) / 100
}
