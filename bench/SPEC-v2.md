# Benchmark spec v2: market-data fan-out

v2 = `SPEC.md` with the changes below. Implementations live in `bench/go-v2` and `bench/rust-v2`
(copies of v1). Everything not mentioned here is unchanged: engine, matching, NPCs, staggered market
makers, HTTP API, event log, slow-client drop at 256 queued messages.

Why: in v1 every order or cancel pushed a full book to every subscriber of the symbol, so push volume grew
with (clients × events), quadratically in load: ~200k messages/s at 10x, which saturated both cores while
matching itself was a few percent of CPU. Real brokers conflate retail market data; so do we.

## 1. Conflated book events (replaces "Book event")

- Processing an order or cancel on symbol K no longer sends anything. It marks K **dirty**.
- Every 100 ms (on the NPC tick, **after** NPC expiry, NPC orders and market makers), for every dirty
  symbol that has at least one subscriber: encode **one** book message (same JSON shape as v1, `ts` =
  time of encoding, microseconds) and enqueue it to each subscriber of K. Then clear all dirty flags
  (dirty symbols without subscribers are just cleared, not encoded).
- So each subscriber gets at most 10 book messages per second per symbol, always the latest state.

## 2. Spread market snapshot (replaces "Market snapshot")

- Every 1000 ms (on every 10th NPC tick, before the book flush of that tick) the engine builds the
  snapshot **once**: `{"t":"mkt","ts":T,"s":[[sym,last,vol], ...]}` containing only the symbols whose
  `last` or `vol` changed since the previous snapshot (`"s":[]` if none).
- Delivery is spread over the next 10 NPC ticks: on tick k, the snapshot is enqueued to the clients whose
  connection id `% 10 == k % 10` (connection ids: one counter, assigned at WS registration).
  A client that connects mid-cycle simply waits for its slot.
- A newly registered client gets one **full** snapshot (all 141 symbols) enqueued immediately.

## 3. Batched writes (wire format change)

- Each client's writer takes **all** messages currently in its queue (at least one, at most 256),
  joins them with `\n` and sends them as **one** WebSocket text frame. A frame therefore contains one or
  more JSON messages separated by newlines (NDJSON). Never wait to accumulate more; just drain what is
  already there.
- Messages are still queued individually (the 256 cap counts messages, not frames).

## 4. Per-connection memory

- Keep per-connection buffers small: target ≤ 64 KB of user-space memory per idle connection
  (read/write buffers of the WebSocket library, queue). Measure RSS with 5000 idle clients and report it.

## Not changed

- Engine is still the single writer. Encoding book/mkt messages may stay on the engine thread (at most
  ~1,400 book encodes/s plus one snapshot/s) — or move off it, implementer's choice, as long as the
  dirty-set semantics above are kept.
- HTTP order/cancel round trip unchanged.
