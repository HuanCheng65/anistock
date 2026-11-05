"""Python implementation of bench/SPEC.md: Starlette + uvicorn (uvloop, httptools) + orjson.

The engine runs on the event loop itself (single writer, no locks). Run:
  uv run --no-project --python 3.13 --with 'uvicorn[standard]' --with starlette --with orjson python server.py
"""

import asyncio
import contextlib
import os
import random
import threading
import time
from bisect import insort
from collections import deque

import orjson
import uvicorn
from starlette.applications import Starlette
from starlette.responses import PlainTextResponse, Response
from starlette.routing import Route, WebSocketRoute
from starlette.websockets import WebSocket, WebSocketDisconnect

NSYM = 141
INIT_CASH = 1_000_000_000_000
INIT_SHARES = 1_000_000
MAX_ACCT = 11140
NPC_RATE = int(os.environ.get("BENCH_NPC_RATE", "300"))
MM_ON = os.environ.get("BENCH_MM", "1") == "1"
LOG_PATH = os.environ.get("BENCH_LOG", "bench-python.log")
QUEUE_CAP = 256


def now_us():
    return time.time_ns() // 1000


class Order:
    __slots__ = ("id", "acct", "sym", "side", "type", "qty", "px", "filled", "ts", "reserve_px", "open")

    def __init__(self, id, acct, sym, side, type_, qty, px, ts, reserve_px):
        self.id, self.acct, self.sym, self.side, self.type = id, acct, sym, side, type_
        self.qty, self.px, self.filled, self.ts, self.reserve_px = qty, px, 0, ts, reserve_px
        self.open = True


class Book:
    """Price levels: dict px -> deque[Order]; sorted price lists (bids stored negated so both ascend)."""
    __slots__ = ("bids", "asks", "bid_px", "ask_px")

    def __init__(self):
        self.bids, self.asks = {}, {}
        self.bid_px, self.ask_px = [], []     # bid_px holds -px

    def add(self, o):
        if o.side == "B":
            q = self.bids.get(o.px)
            if q is None:
                q = self.bids[o.px] = deque()
                insort(self.bid_px, -o.px)
        else:
            q = self.asks.get(o.px)
            if q is None:
                q = self.asks[o.px] = deque()
                insort(self.ask_px, o.px)
        q.append(o)

    def depth(self):
        bids = []
        for npx in self.bid_px[:5]:
            bids.append([-npx, sum(o.qty - o.filled for o in self.bids[-npx])])
        asks = []
        for px in self.ask_px[:5]:
            asks.append([px, sum(o.qty - o.filled for o in self.asks[px])])
        return bids, asks


class Engine:
    def __init__(self, log):
        self.base = [1000 + 100 * s for s in range(NSYM)]
        self.last = list(self.base)
        self.vol = [0] * NSYM
        self.books = [Book() for _ in range(NSYM)]
        self.cash = {}            # acct -> [cash, frozen]
        self.hold = {}            # (acct, sym) -> [shares, frozen]
        self.orders = {}          # id -> open Order
        self.next_id = 1
        self.subs = [set() for _ in range(NSYM)]
        self.clients = set()
        self.n_orders = self.n_npc = self.n_trades = self.dropped = 0
        self.npc_fifo = deque()
        self.mm_orders = [(0, 0)] * NSYM
        self.tick = 0
        self.log = log
        self.rng = random.Random()

    def acct(self, a):
        c = self.cash.get(a)
        if c is None:
            c = self.cash[a] = [INIT_CASH, 0]
        return c

    def holding(self, a, s):
        k = (a, s)
        h = self.hold.get(k)
        if h is None:
            h = self.hold[k] = [INIT_SHARES, 0]
        return h

    # ---- orders -------------------------------------------------------------------------------
    def place(self, acct, sym, side, type_, qty, px):
        """Returns (order, None) or (None, error). Emits the book event."""
        if not (0 <= acct <= MAX_ACCT) or not (0 <= sym < NSYM) or not (1 <= qty <= 100_000):
            return None, "bad field"
        if side not in ("B", "S") or type_ not in ("L", "M"):
            return None, "bad side/type"
        last = self.last[sym]
        if type_ == "L":
            if not isinstance(px, int) or not (last // 2 <= px <= last * 2):
                return None, "price out of band"
            reserve_px = px
        else:
            px = 0
            reserve_px = last * 2
        if side == "B":
            c = self.acct(acct)
            need = qty * reserve_px
            if c[0] - c[1] < need:
                return None, "insufficient cash"
            c[1] += need
        else:
            h = self.holding(acct, sym)
            if h[0] - h[1] < qty:
                return None, "insufficient shares"
            h[1] += qty
        ts = now_us()
        o = Order(self.next_id, acct, sym, side, type_, qty, px, ts, reserve_px)
        self.next_id += 1
        self.log.append(orjson.dumps({"e": "o", "id": o.id, "acct": acct, "sym": sym, "side": side, "type": type_,
                                      "qty": qty, "px": px, "ts": ts}))
        self.match(o)
        if o.open:
            if o.type == "L":
                self.books[sym].add(o)
                self.orders[o.id] = o
            else:
                self.finish(o)
        self.emit(sym)
        return o, None

    def match(self, o):
        book = self.books[o.sym]
        if o.side == "B":
            levels, prices, sign = book.asks, book.ask_px, 1
        else:
            levels, prices, sign = book.bids, book.bid_px, -1
        while o.filled < o.qty and prices:
            px = prices[0] * sign
            if o.type == "L" and (px > o.px if o.side == "B" else px < o.px):
                break
            q = levels[px]
            rest = q[0]
            if rest.acct == o.acct:
                self.finish(o)          # self-trade prevention: cancel the incoming remainder
                return
            n = min(o.qty - o.filled, rest.qty - rest.filled)
            if o.side == "B":
                self.settle(o, rest, px, n)
            else:
                self.settle(rest, o, px, n)
            o.filled += n
            rest.filled += n
            if rest.filled == rest.qty:
                rest.open = False
                q.popleft()
                del self.orders[rest.id]
                if not q:
                    del levels[px]
                    prices.pop(0)
        if o.filled == o.qty:
            o.open = False

    def settle(self, buy, sell, px, n):
        bc = self.acct(buy.acct)
        bc[0] -= n * px
        bc[1] -= n * buy.reserve_px
        self.acct(sell.acct)[0] += n * px
        sh = self.holding(sell.acct, sell.sym)
        sh[0] -= n
        sh[1] -= n
        self.holding(buy.acct, buy.sym)[0] += n
        self.last[buy.sym] = px
        self.vol[buy.sym] += n
        self.n_trades += 1
        self.log.append(orjson.dumps({"e": "t", "sym": buy.sym, "px": px, "qty": n, "buy": buy.id, "sell": sell.id,
                                      "ts": now_us()}))

    def finish(self, o):
        """Order ends with a remainder: release what is still frozen."""
        o.open = False
        left = o.qty - o.filled
        if left:
            if o.side == "B":
                self.acct(o.acct)[1] -= left * o.reserve_px
            else:
                self.holding(o.acct, o.sym)[1] -= left

    def cancel(self, acct, oid):
        o = self.orders.get(oid)
        if o is None or o.acct != acct:
            return False
        del self.orders[oid]
        book = self.books[o.sym]
        if o.side == "B":
            levels, prices, key = book.bids, book.bid_px, -o.px
        else:
            levels, prices, key = book.asks, book.ask_px, o.px
        q = levels[o.px]
        q.remove(o)
        if not q:
            del levels[o.px]
            prices.remove(key)
        self.finish(o)
        self.log.append(orjson.dumps({"e": "c", "id": oid, "ts": now_us()}))
        self.emit(o.sym)
        return True

    # ---- broadcast ----------------------------------------------------------------------------
    def emit(self, sym):
        subs = self.subs[sym]
        if not subs:
            return
        bids, asks = self.books[sym].depth()
        msg = orjson.dumps({"t": "book", "sym": sym, "last": self.last[sym], "vol": self.vol[sym],
                            "bids": bids, "asks": asks, "ts": now_us()}).decode()
        self.send(subs, msg)

    def send(self, clients, msg):
        for cl in list(clients):
            q = cl.queue
            if q.full():
                self.drop(cl)
            else:
                q.put_nowait(msg)

    def drop(self, cl):
        if cl in self.clients:
            self.dropped += 1
            self.remove(cl)
            cl.queue.put_nowait_force(None)

    def remove(self, cl):
        self.clients.discard(cl)
        self.subs[cl.sym].discard(cl)

    def snapshot(self):
        msg = orjson.dumps({"t": "mkt", "ts": now_us(),
                            "s": [[s, self.last[s], self.vol[s]] for s in range(NSYM)]}).decode()
        self.send(self.clients, msg)

    # ---- NPCs ---------------------------------------------------------------------------------
    def ref(self, s):
        return (8 * self.last[s] + 2 * self.base[s]) // 10

    def npc_tick(self):
        r = self.rng
        cutoff = time.monotonic() - 30
        fifo = self.npc_fifo
        while fifo and fifo[0][0] <= cutoff:
            _, oid, a = fifo.popleft()
            self.cancel(a, oid)
        for _ in range(NPC_RATE // 10):
            a = r.randint(10000, 10999)
            s = r.randrange(NSYM)
            side = "B" if r.random() < 0.5 else "S"
            if r.random() < 0.8:
                px = round(self.ref(s) * (1 + r.uniform(-0.02, 0.02)))
                o, _ = self.place(a, s, side, "L", r.randint(1, 100), px)
                if o is not None and o.open:
                    self.npc_fifo.append((time.monotonic(), o.id, a))
            else:
                o, _ = self.place(a, s, side, "M", r.randint(1, 50), 0)
            if o is not None:
                self.n_npc += 1
        if MM_ON:
            k = self.tick % 10
            for s in range(k, NSYM, 10):
                self.mm_quote(s)
        self.tick += 1

    def mm_quote(self, s):
        a = 11000 + s
        bid, ask = self.mm_orders[s]
        self.cancel(a, bid)
        self.cancel(a, ask)
        ref = self.ref(s)
        b, _ = self.place(a, s, "B", "L", 200, ref * 99 // 100)
        k, _ = self.place(a, s, "S", "L", 200, ref * 101 // 100)
        self.n_npc += (b is not None) + (k is not None)
        self.mm_orders[s] = (b.id if b else 0, k.id if k else 0)


class Client:
    __slots__ = ("sym", "queue")

    def __init__(self, sym):
        self.sym = sym
        self.queue = BoundedQueue(QUEUE_CAP)


class BoundedQueue(asyncio.Queue):
    def put_nowait_force(self, item):
        """Wake the writer with a sentinel even when full."""
        self._queue.append(item)
        self._unfinished_tasks += 1
        self._wakeup_next(self._getters)


# ---- event log ------------------------------------------------------------------------------------
class Log:
    """The engine appends bytes to a list; a thread swaps it out, writes, fdatasyncs every 50 ms."""

    def __init__(self, path):
        self.buf = []
        self.lock = threading.Lock()
        self.f = open(path, "wb")
        threading.Thread(target=self.run, daemon=True).start()

    def append(self, line):
        self.buf.append(line)       # list.append is atomic under the GIL

    def run(self):
        while True:
            time.sleep(0.05)
            buf, self.buf = self.buf, []
            if buf:
                self.f.write(b"\n".join(buf) + b"\n")
                self.f.flush()
                os.fdatasync(self.f.fileno())


ENGINE: Engine = None  # set at startup


def err(msg):
    return Response(orjson.dumps({"error": msg}), status_code=400, media_type="application/json")


async def health(_):
    return PlainTextResponse("ok")


async def order(request):
    try:
        b = orjson.loads(await request.body())
        o, e = ENGINE.place(int(b["acct"]), int(b["sym"]), b["side"], b["type"], int(b["qty"]), b.get("px") or 0)
    except (KeyError, ValueError, TypeError, orjson.JSONDecodeError):
        return err("bad request")
    if e:
        return err(e)
    status = "filled" if o.filled == o.qty else ("open" if o.open else "cancelled")
    ENGINE.n_orders += 1
    return Response(orjson.dumps({"id": o.id, "filled": o.filled, "status": status}), media_type="application/json")


async def cancel(request):
    try:
        b = orjson.loads(await request.body())
        ok = ENGINE.cancel(int(b["acct"]), int(b["id"]))
    except (KeyError, ValueError, TypeError, orjson.JSONDecodeError):
        return err("bad request")
    return Response(orjson.dumps({"ok": ok}), media_type="application/json")


async def stats(_):
    e = ENGINE
    return Response(orjson.dumps({"orders": e.n_orders, "npc": e.n_npc, "trades": e.n_trades, "ws": len(e.clients),
                                  "dropped": e.dropped}), media_type="application/json")


async def ws(socket: WebSocket):
    try:
        sym = int(socket.query_params.get("sym", "0"))
    except ValueError:
        sym = 0
    sym = min(max(sym, 0), NSYM - 1)
    await socket.accept()
    cl = Client(sym)
    ENGINE.clients.add(cl)
    ENGINE.subs[sym].add(cl)

    async def drain_incoming():
        try:
            while True:
                await socket.receive_text()
        except (WebSocketDisconnect, RuntimeError):
            cl.queue.put_nowait_force(None)

    reader = asyncio.create_task(drain_incoming())
    try:
        while True:
            msg = await cl.queue.get()
            if msg is None:
                break
            await socket.send_text(msg)
    except (WebSocketDisconnect, RuntimeError, OSError):
        pass
    finally:
        reader.cancel()
        ENGINE.remove(cl)
        try:
            await socket.close()
        except Exception:
            pass


async def every(period, fn):
    nxt = time.monotonic()
    while True:
        nxt += period
        await asyncio.sleep(max(0.0, nxt - time.monotonic()))
        fn()


@contextlib.asynccontextmanager
async def lifespan(app):
    global ENGINE
    ENGINE = Engine(Log(LOG_PATH))
    tasks = [asyncio.create_task(every(0.1, ENGINE.npc_tick)), asyncio.create_task(every(1.0, ENGINE.snapshot))]
    yield
    for t in tasks:
        t.cancel()


app = Starlette(routes=[Route("/health", health), Route("/order", order, methods=["POST"]),
                        Route("/cancel", cancel, methods=["POST"]), Route("/stats", stats),
                        WebSocketRoute("/ws", ws)], lifespan=lifespan)

if __name__ == "__main__":
    uvicorn.run(app, host="127.0.0.1", port=int(os.environ.get("BENCH_PORT", "18700")), loop="uvloop",
                http="httptools", ws="websockets-sansio", log_level="warning", access_log=False)
