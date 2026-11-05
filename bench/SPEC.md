# AniStock v2 core benchmark — server spec

Three implementations (Python, Go, Rust) of the **same** minimal exchange core, driven by one load
generator (`bench/loadgen`, Go). The question: which language holds p99 < 20 ms at several times
the expected load on a 2-core box, with NPC traders running inside the server.

Everything below is normative. Do not add features, caching or coalescing that the other
implementations do not have. Where the spec says "free", pick whatever is idiomatic.

## Process

- Listen on `127.0.0.1:$BENCH_PORT` (default 18700). HTTP/1.1 keep-alive.
- **Single writer**: all exchange state (books, accounts, ids, counters) is mutated by exactly one
  thread / goroutine / event loop ("the engine"). HTTP parsing, JSON decoding of requests and
  WebSocket writes may run elsewhere. Requests reach the engine through a queue/channel with a reply
  channel (Go, Rust) or run on the event loop directly (Python).
- Thread counts: Go honours `GOMAXPROCS`, Rust uses a tokio multi-thread runtime whose worker count is
  `TOKIO_WORKER_THREADS` (default 2) plus the engine thread and the log thread. Python: one process,
  uvloop.

## State

- Symbols `0..140` (141). `base[s] = 1000 + 100*s` (cents). `last[s] = base[s]` at start; `vol[s] = 0`.
- Accounts are integers. Players `0..9999`, NPC agents `10000..10999`, market maker of symbol s is
  `11000 + s`. Every account starts with `cash = 1_000_000_000_000` and `cash_frozen = 0`.
- Holdings `(acct, sym)` are created on first touch with `shares = 1_000_000`, `shares_frozen = 0`.
- Order ids: one global counter starting at 1.

## Orders

Order fields: `id, acct, sym, side (B|S), type (L|M), qty, px (limit only), filled, ts`.

Validation (reject with HTTP 400 `{"error":"..."}`):
- `acct` in `0..11140`, `sym` in `0..140`, `qty` in `1..100000`, side/type valid.
- Limit: `px` integer with `last[sym]/2 <= px <= last[sym]*2` (integer division for the lower bound).
- Buy: reserve = `qty*px` (limit) or `qty*last[sym]*2` (market); require `cash - cash_frozen >= reserve`.
- Sell: require `shares - shares_frozen >= qty`.
- On acceptance freeze the reserve (buy) or qty (sell).

Matching, per symbol, price-time priority:
- Incoming buy matches the lowest ask while `type == M` or `ask.px <= px`; sell mirrors it with the
  highest bid. Trade price = **resting** order's price. Trade qty = min of the two remaining.
- Self-trade prevention: if the best resting order on the other side belongs to the same account,
  stop matching and **cancel the incoming order's remainder** (status `cancelled`).
- Market order remainder is cancelled. Limit remainder rests in the book.
- Settlement per trade (qty q at price p): buyer `cash -= q*p`, buyer `cash_frozen -= q*p_reserve`
  where `p_reserve` is the buy order's px (limit) or `last_at_entry*2` (market; remember it on the
  order); seller `cash += q*p`; seller `shares -= q`, `shares_frozen -= q`; buyer `shares += q`.
  `last[sym] = p`, `vol[sym] += q`.
- When a buy order ends (filled, cancelled, remainder cancelled) release its remaining frozen cash;
  sell orders release remaining frozen shares.
- Book data structure is free (sorted price levels with FIFO queues is the obvious choice), but
  matching, cancel and depth must not be O(total orders in the book) per operation.

Cancel: by `(acct, id)`. Succeeds only if the order exists, is open and belongs to `acct`.

## HTTP API (JSON bodies)

- `GET /health` → `ok`
- `POST /order` `{"acct":1,"sym":5,"side":"B","type":"L","qty":10,"px":1500}` (px omitted or 0 for M)
  → `{"id":123,"filled":10,"status":"filled"|"open"|"cancelled"}`
- `POST /cancel` `{"acct":1,"id":123}` → `{"ok":true|false}`
- `GET /stats` → `{"orders":N,"npc":N,"trades":N,"ws":N,"dropped":N}`
  (`orders` = accepted HTTP orders, `npc` = orders placed by NPCs and market makers,
  `ws` = connected WebSocket clients, `dropped` = clients dropped as slow)

## WebSocket

- `GET /ws?sym=K` upgrades. The client never sends anything (ignore anything it sends).
- **Book event**: after the engine processes any order or cancel (HTTP, NPC or MM) on symbol K, send to
  every subscriber of K one text message, encoded **once** and shared by all subscribers:
  `{"t":"book","sym":K,"last":L,"vol":V,"bids":[[px,qty],...],"asks":[[px,qty],...],"ts":T}`
  with up to 5 price levels per side, qty aggregated per level, best first. `T` = Unix time in
  **microseconds** taken when the event is created.
  An order that rejects (400) emits nothing. A failed cancel emits nothing.
- **Market snapshot**: every 1000 ms, to every client:
  `{"t":"mkt","ts":T,"s":[[sym,last,vol], ... all 141]}` (encoded once).
- Slow clients: each client has an outbound queue of capacity **256** messages. If a message
  cannot be enqueued because it is full, drop the client (close the connection) and count it in
  `dropped`. Never block the engine on a client.

## NPC traders (inside the server)

Configured by env: `BENCH_NPC_RATE` (orders per second, default 300) and `BENCH_MM` (1 = market makers
on, default 1). RNG is free (seeded or not).

- Every 100 ms the engine runs an NPC tick: place `BENCH_NPC_RATE / 10` orders (integer). For each:
  account uniform in `10000..10999`, symbol uniform in `0..140`, side 50/50,
  `ref = (8*last[sym] + 2*base[sym]) / 10` (integer division).
  80%: limit, `px = round(ref * (1 + U(-0.02, 0.02)))`, qty uniform `1..100`.
  20%: market, qty uniform `1..50`.
  Orders go through the same validation, matching and book events as HTTP orders.
- NPC limit orders still open 30 s after placement are cancelled (check in the NPC tick; keep a FIFO
  of `(placed_at, id)` so the check is not a full book scan). Cancels emit book events.
- Market makers, **staggered**: the NPC tick also re-quotes one tenth of the symbols. Number the NPC ticks
  `k = 0, 1, 2, ...`; tick k handles every symbol with `sym % 10 == k % 10` (so each symbol is re-quoted once
  per second, spread evenly). For each such symbol: cancel that symbol's MM bid and ask from its last round
  (if still open), then place a limit bid at `ref*99/100` and a limit ask at `ref*101/100`
  (integer division, ref as above), qty 200 each, account `11000 + sym`. Order within a tick: NPC expiry,
  NPC orders, then market makers. (Re-quoting all 141 symbols at once was the first draft; that single
  ~850-event burst per second dominated tail latency in every language, and no real system would do it.)

## Event log (persistence)

Every accepted order, every trade and every successful cancel is appended to a log as one JSON line:
`{"e":"o","id":..,"acct":..,"sym":..,"side":"B","type":"L","qty":..,"px":..,"ts":..}`,
`{"e":"t","sym":..,"px":..,"qty":..,"buy":<order id>,"sell":<order id>,"ts":..}`,
`{"e":"c","id":..,"ts":..}`.
The engine hands lines to a separate log writer (thread / goroutine) through a queue; the writer
appends them to `$BENCH_LOG` (default `bench-<lang>.log` in the working directory, truncated at
start) and calls `fdatasync` **every 50 ms** if anything was written (group commit). The engine never
waits on disk.

## Not included

VCM, auctions, IPO, fees, auth, sessions, rate limits. They cost roughly the same in every language
and would only add noise.
