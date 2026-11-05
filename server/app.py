"""AniStock game server.

    uv run uvicorn server.app:app --host 0.0.0.0 --port 8080

Env: ANISTOCK_SECRET (cookie signing, required in production), ANISTOCK_INVITE (club invite code),
ANISTOCK_DB / ANISTOCK_GAME_DB (paths), ANISTOCK_ALWAYS_OPEN=1 (ignore trading hours, for testing),
ANISTOCK_NO_LOOPS=1 (no NPC / daily background tasks, for testing).
"""

from __future__ import annotations

import asyncio
import hashlib
import hmac
import json
import logging
import os
import random
import secrets
import sqlite3
import subprocess
import sys
import time
from contextlib import asynccontextmanager
from datetime import date, datetime, timedelta
from decimal import Decimal, InvalidOperation
from pathlib import Path

from fastapi import Depends, FastAPI, HTTPException, Request, Response
from fastapi.responses import FileResponse, JSONResponse
from fastapi.staticfiles import StaticFiles
from pydantic import BaseModel, Field

from . import daily, npc
from . import db as gdb
from . import exchange as ex
from .config import ROOT, Clock, load, next_daily_step

log = logging.getLogger("server")
cfg = load()
clock = Clock(cfg.tz)
LOCK = asyncio.Lock()
conn: sqlite3.Connection | None = None
ALWAYS_OPEN = os.environ.get("ANISTOCK_ALWAYS_OPEN") == "1"
SECRET = cfg.secret      # replaced at startup by a random key stored in the game DB if ANISTOCK_SECRET is unset
SESSION_DAYS = 30


# ---------------------------------------------------------------- helpers

def db() -> sqlite3.Connection:
    assert conn is not None
    return conn


def fees() -> ex.Fees:
    return ex.Fees(commission=cfg.game["commission"], stamp_duty=cfg.game["stamp_duty"])


RULES = npc.rules_of(cfg)


def phase() -> str:
    """auction (集合竞价, orders rest) | continuous | closed."""
    if ALWAYS_OPEN:
        return "continuous"
    t = clock.now().strftime("%H:%M")
    g = cfg.game
    if g["auction_start"] <= t < g["trading_open"]:
        return "auction"
    if g["trading_open"] <= t < g["trading_close"]:
        return "continuous"
    return "closed"


def trading_open() -> bool:
    return phase() != "closed"


def ensure_opening_auction(c) -> None:
    """Once a day, when continuous trading starts: clear every stock's opening auction and set the
    day's anchor price for the volatility control. Caller holds LOCK and an open transaction."""
    today = clock.today()
    if gdb.get_state(c, "auction_day") == today:
        return
    for co in c.execute("SELECT * FROM companies WHERE status='listed'").fetchall():
        price = ex.run_auction(c, co["id"], day=today, ts=clock.ts(), fees=fees())
        c.execute("UPDATE companies SET open_ref=?, vcm_until=NULL, vcm_count=0 WHERE id=?",
                  (price or co["prev_close"], co["id"]))
    gdb.set_state(c, "auction_day", today)


def yuan(cents: int | None) -> float | None:
    return None if cents is None else round(cents / 100, 2)


def to_cents(value) -> int:
    try:
        d = Decimal(str(value))
    except InvalidOperation:
        raise HTTPException(400, "价格格式不对")
    if d <= 0 or d != d.quantize(Decimal("0.01")):
        raise HTTPException(400, "价格最多两位小数，且必须大于 0")
    return int(d * 100)


# ---------------------------------------------------------------- auth

def hash_pw(pw: str, salt: bytes | None = None) -> str:
    salt = salt or secrets.token_bytes(16)
    h = hashlib.scrypt(pw.encode(), salt=salt, n=2 ** 14, r=8, p=1)
    return salt.hex() + ":" + h.hex()


def check_pw(pw: str, stored: str) -> bool:
    salt, h = stored.split(":")
    return hmac.compare_digest(hash_pw(pw, bytes.fromhex(salt)).split(":")[1], h)


def make_session(account_id: int) -> str:
    exp = int(time.time()) + SESSION_DAYS * 86400
    payload = f"{account_id}.{exp}"
    sig = hmac.new(SECRET, payload.encode(), hashlib.sha256).hexdigest()
    return f"{payload}.{sig}"


def read_session(token: str | None) -> int | None:
    if not token:
        return None
    try:
        aid, exp, sig = token.split(".")
    except ValueError:
        return None
    good = hmac.new(SECRET, f"{aid}.{exp}".encode(), hashlib.sha256).hexdigest()
    if not hmac.compare_digest(sig, good) or int(exp) < time.time():
        return None
    return int(aid)


def current_player(request: Request) -> sqlite3.Row:
    aid = read_session(request.cookies.get("session"))
    row = db().execute("SELECT * FROM accounts WHERE id=? AND kind='player'", (aid,)).fetchone() if aid else None
    if row is None:
        raise HTTPException(401, "请先登录")
    return row


def optional_player(request: Request) -> sqlite3.Row | None:
    try:
        return current_player(request)
    except HTTPException:
        return None


def set_cookie(resp: Response, account_id: int) -> None:
    resp.set_cookie("session", make_session(account_id), max_age=SESSION_DAYS * 86400, httponly=True, samesite="lax")


# ---------------------------------------------------------------- background loops

async def npc_loop() -> None:
    rng = random.Random()
    while True:
        await asyncio.sleep(cfg.npc["tick_seconds"])
        ph = phase()
        if ph == "closed":
            continue
        async with LOCK:
            try:
                c = db()
                c.execute("BEGIN IMMEDIATE")
                if ph == "continuous":
                    ensure_opening_auction(c)
                npc.tick(c, cfg, clock.today(), clock.ts(), rng, auction=(ph == "auction"))
                c.execute("COMMIT")
            except Exception:
                log.exception("npc tick failed")
                db().execute("ROLLBACK")


def refresh_inputs() -> None:
    """Rebuild the derived market series before a close: refresh Annict air times (~10 requests), rebuild.
    Matching new shows across platforms (sim/match.py --search) is a season-start task: it rewrites
    tracked files, so it is run by hand and committed, never by the server."""
    py = sys.executable
    for args in (["sim/reconstruct.py", "airtimes"], ["sim/reconstruct.py", "build"]):
        try:
            r = subprocess.run([py, *args], cwd=ROOT, capture_output=True, text=True, timeout=900)
            if r.returncode:
                log.warning("%s failed: %s", " ".join(args), r.stderr[-500:])
        except Exception:
            log.exception("%s failed", " ".join(args))


def catch_up() -> list[dict]:
    """Close every day from the last closed one up to yesterday (uses its own connection)."""
    c = gdb.connect(cfg.game_db)
    out = []
    last = gdb.get_state(c, "last_closed")
    d = date.fromisoformat(last) + timedelta(days=1)
    yesterday = date.fromisoformat(clock.today()) - timedelta(days=1)
    while d <= yesterday:
        out.append(daily.run_day(c, cfg, d.isoformat(), clock.ts()))
        d += timedelta(days=1)
    c.close()
    return out


async def daily_loop() -> None:
    while True:
        last = gdb.get_state(db(), "last_closed")
        yesterday = (date.fromisoformat(clock.today()) - timedelta(days=1)).isoformat()
        if last < yesterday and clock.now().time() >= datetime.strptime(cfg.game["daily_step_at"], "%H:%M").time():
            async with LOCK:
                await asyncio.to_thread(refresh_inputs)
                try:
                    res = await asyncio.to_thread(catch_up)
                    log.info("daily close: %s", res)
                except Exception:
                    log.exception("daily close failed")
        wait = (next_daily_step(clock.now(), cfg.game["daily_step_at"]) - clock.now()).total_seconds()
        await asyncio.sleep(max(60, min(wait, 3600)))


@asynccontextmanager
async def lifespan(_app: FastAPI):
    global conn, SECRET
    if not cfg.game_db.exists():
        raise SystemExit(f"{cfg.game_db} missing: run `uv run python -m server.seed` first")
    conn = gdb.connect(cfg.game_db)
    if cfg.secret == b"dev-only-secret-change-me":
        # never sign sessions with a key that is in the source code: keep a random one in the game DB
        key = gdb.get_state(conn, "session_secret")
        if not key:
            key = secrets.token_hex(32)
            gdb.set_state(conn, "session_secret", key)
        SECRET = bytes.fromhex(key)
        log.warning("ANISTOCK_SECRET is not set: using a random key stored in the game database")
    tasks = [] if os.environ.get("ANISTOCK_NO_LOOPS") == "1" else [asyncio.create_task(npc_loop()), asyncio.create_task(daily_loop())]
    yield
    for t in tasks:
        t.cancel()
    conn.close()


app = FastAPI(title="AniStock", lifespan=lifespan)


@app.exception_handler(ex.OrderError)
async def order_error(_req, exc: ex.OrderError):
    return JSONResponse({"detail": str(exc)}, status_code=400)


# ---------------------------------------------------------------- account

class Credentials(BaseModel):
    username: str = Field(min_length=2, max_length=20)
    password: str = Field(min_length=6, max_length=100)
    invite: str | None = None


@app.post("/api/register")
async def register(body: Credentials, response: Response):
    if body.invite != cfg.invite_code:
        raise HTTPException(400, "邀请码不对，请向社团管理员索取")
    name = body.username.strip()
    async with LOCK:
        c = db()
        if c.execute("SELECT 1 FROM accounts WHERE username=?", (name,)).fetchone():
            raise HTTPException(400, "这个用户名已经有人用了")
        c.execute("BEGIN IMMEDIATE")
        aid = c.execute("INSERT INTO accounts(kind, name, username, pw_hash, created_at) VALUES ('player', ?, ?, ?, ?)",
                        (name, name, hash_pw(body.password), clock.ts())).lastrowid
        ex.transfer(c, gdb.special(c, "world"), aid, round(cfg.game["initial_cash"] * 100), "grant",
                    day=clock.today(), ts=clock.ts())
        c.execute("COMMIT")
    set_cookie(response, aid)
    return {"ok": True}


@app.post("/api/login")
async def login(body: Credentials, response: Response):
    row = db().execute("SELECT * FROM accounts WHERE username=? AND kind='player'", (body.username.strip(),)).fetchone()
    if row is None or not check_pw(body.password, row["pw_hash"]):
        raise HTTPException(400, "用户名或密码不对")
    set_cookie(response, row["id"])
    return {"ok": True}


@app.post("/api/logout")
async def logout(response: Response):
    response.delete_cookie("session")
    return {"ok": True}


def portfolio_rows(account_id: int) -> list[dict]:
    c = db()
    out = []
    for h in c.execute("""SELECT h.company_id, h.qty, h.qty_frozen, co.code, co.title, co.title_ja, co.last_price, co.prev_close, co.status
                          FROM holdings h JOIN companies co ON co.id = h.company_id
                          WHERE h.account_id=? AND h.qty > 0 ORDER BY co.code""", (account_id,)).fetchall():
        cost = c.execute("SELECT COALESCE(SUM(qty*cost),0) FROM lots WHERE account_id=? AND company_id=?",
                         (account_id, h["company_id"])).fetchone()[0]
        divs = c.execute("SELECT COALESCE(SUM(amount),0) FROM ledger WHERE to_id=? AND company_id=? AND reason='dividend'",
                         (account_id, h["company_id"])).fetchone()[0]
        oldest = c.execute("SELECT MIN(acquired_on) FROM lots WHERE account_id=? AND company_id=?",
                           (account_id, h["company_id"])).fetchone()[0]
        value = h["qty"] * h["last_price"]
        out.append({
            "company_id": h["company_id"], "code": h["code"], "title": h["title"], "title_ja": h["title_ja"], "qty": h["qty"],
            "available": h["qty"] - h["qty_frozen"], "avg_cost": yuan(round(cost / h["qty"])),
            "price": yuan(h["last_price"]), "value": yuan(value), "pnl": yuan(value - cost),
            "pnl_pct": (value - cost) / cost if cost else None, "dividends": yuan(divs),
            "day_change": (h["last_price"] - h["prev_close"]) / h["prev_close"] if h["prev_close"] else 0,
            "oldest_lot": oldest,
        })
    return out


def totals(account_id: int) -> dict:
    a = db().execute("SELECT * FROM accounts WHERE id=?", (account_id,)).fetchone()
    stock = db().execute("""SELECT COALESCE(SUM(h.qty * co.last_price),0) FROM holdings h
                            JOIN companies co ON co.id=h.company_id WHERE h.account_id=?""", (account_id,)).fetchone()[0]
    return {"cash": yuan(a["cash"]), "frozen": yuan(a["cash_frozen"]), "available": yuan(a["cash"] - a["cash_frozen"]),
            "stock_value": yuan(stock), "total": yuan(a["cash"] + stock)}


@app.get("/api/me")
async def me(player=Depends(current_player)):
    t = totals(player["id"])
    initial = cfg.game["initial_cash"]
    rate = float(gdb.get_state(db(), "rate"))
    divs = db().execute("SELECT COALESCE(SUM(amount),0) FROM ledger WHERE to_id=? AND reason='dividend'",
                        (player["id"],)).fetchone()[0]
    return {"id": player["id"], "username": player["username"], **t, "initial": initial,
            "return_pct": (t["total"] - initial) / initial, "dividends": yuan(divs),
            "deposit_rate": max(0.0, rate - cfg.params["central_bank"]["deposit_spread"])}


@app.get("/api/portfolio")
async def portfolio(player=Depends(current_player)):
    return {"positions": portfolio_rows(player["id"]), **totals(player["id"])}


# ---------------------------------------------------------------- market data

@app.get("/api/status")
async def status():
    c = db()
    rate = float(gdb.get_state(c, "rate"))
    return {
        "now": clock.ts(), "day": clock.today(), "trading": trading_open(), "phase": phase(),
        "auction_start": cfg.game["auction_start"], "open": cfg.game["trading_open"], "close": cfg.game["trading_close"],
        "last_closed": gdb.get_state(c, "last_closed"), "launched_on": gdb.get_state(c, "launched_on"),
        "rate": rate, "deposit_rate": max(0.0, rate - cfg.params["central_bank"]["deposit_spread"]),
        "rules": {"vcm_threshold": cfg.game["vcm_threshold"], "vcm_window_min": cfg.game["vcm_window_min"],
                  "vcm_cool_min": cfg.game["vcm_cool_min"],
                  "commission": cfg.game["commission"], "stamp_duty": cfg.game["stamp_duty"],
                  "dividend_tax": cfg.game["dividend_tax"], "initial_cash": cfg.game["initial_cash"]},
    }


def _week_revenue(company_id: int, end: str) -> tuple[int, int]:
    d = date.fromisoformat(end)
    c = db()
    this = c.execute("SELECT COALESCE(SUM(revenue),0) FROM company_days WHERE company_id=? AND day>? AND day<=?",
                     (company_id, (d - timedelta(days=7)).isoformat(), end)).fetchone()[0]
    prev = c.execute("SELECT COALESCE(SUM(revenue),0) FROM company_days WHERE company_id=? AND day>? AND day<=?",
                     (company_id, (d - timedelta(days=14)).isoformat(), (d - timedelta(days=7)).isoformat())).fetchone()[0]
    return this, prev


@app.get("/api/market")
async def market():
    c = db()
    today = clock.today()
    last_closed = gdb.get_state(c, "last_closed")
    rows = []
    vol = {r["company_id"]: (r["v"], r["t"]) for r in c.execute(
        "SELECT company_id, SUM(qty) v, SUM(qty*price) t FROM trades WHERE day=? GROUP BY company_id", (today,))}
    for co in c.execute("SELECT * FROM companies WHERE status='listed' ORDER BY code").fetchall():
        this, prev = _week_revenue(co["id"], last_closed)
        aired = c.execute("SELECT aired FROM company_days WHERE company_id=? ORDER BY day DESC LIMIT 1", (co["id"],)).fetchone()
        spark = [r["close"] for r in c.execute(
            "SELECT close FROM (SELECT day, close FROM candles WHERE company_id=? ORDER BY day DESC LIMIT 20) ORDER BY day",
            (co["id"],))]
        ipo_open = c.execute("""SELECT o.qty - o.filled AS left FROM orders o JOIN accounts a ON a.id=o.account_id
                                WHERE o.company_id=? AND o.status='open' AND a.kind='company'""", (co["id"],)).fetchone()
        v, t = vol.get(co["id"], (0, 0))
        rows.append({
            "id": co["id"], "code": co["code"], "title": co["title"], "title_ja": co["title_ja"], "title_en": co["title_en"],
            "price": yuan(co["last_price"]), "prev_close": yuan(co["prev_close"]),
            "change": (co["last_price"] - co["prev_close"]) / co["prev_close"] if co["prev_close"] else 0,
            "volume": v or 0, "turnover": yuan(t or 0),
            "market_cap": yuan(co["last_price"] * co["shares"]),
            "week_revenue": yuan(this), "revenue_change": (this - prev) / prev if prev else None,
            "aired": aired["aired"] if aired else 0, "episodes": co["episodes"], "long_runner": bool(co["long_runner"]),
            "first_air": co["first_air"], "listed_on": co["listed_on"],
            "ipo_left": ipo_open["left"] if ipo_open else 0, "ipo_price": yuan(co["ipo_price"]),
            "new": (date.fromisoformat(today) - date.fromisoformat(co["listed_on"])).days < 5,
            "cooling": ex.cooling(co, clock.ts()) is not None,
            "spark": [yuan(x) for x in spark],
        })
    return {"stocks": rows}


def _audiences(company_id: int) -> dict:
    """Public audience numbers from the collector (aggregates only)."""
    out = {}
    try:
        cdb = sqlite3.connect(f"file:{cfg.collector_db}?mode=ro", uri=True)
        r = cdb.execute("SELECT data FROM snapshots WHERE source='anilist' AND kind='media' AND external_id=? ORDER BY day DESC LIMIT 1",
                        (str(company_id),)).fetchone()
        if r:
            m = json.loads(r[0])
            out["anilist"] = {"popularity": m.get("popularity"), "score": m.get("averageScore"),
                              "next_episode": m.get("nextAiringEpisode"), "genres": m.get("genres", [])[:4],
                              "cover": (m.get("coverImage") or {}).get("large")}
        matches = json.loads((ROOT / "sim" / "matches.json").read_text(encoding="utf-8"))["matches"].get(str(company_id), {})
        if matches.get("bgm"):
            r = cdb.execute("SELECT data FROM snapshots WHERE source='bgm' AND kind='subject' AND external_id=? ORDER BY day DESC LIMIT 1",
                            (str(matches["bgm"]),)).fetchone()
            if r:
                s = json.loads(r[0])
                out["bangumi"] = {"collection": s.get("collection"), "score": (s.get("rating") or {}).get("score"),
                                  "name_cn": s.get("name_cn"), "id": matches["bgm"]}
        if matches.get("annict"):
            r = cdb.execute("SELECT data FROM snapshots WHERE source='annict' AND kind='work' AND external_id=? ORDER BY day DESC LIMIT 1",
                            (str(matches["annict"]),)).fetchone()
            if r:
                w = json.loads(r[0])
                out["annict"] = {"watchers": w.get("watchersCount"), "id": matches["annict"]}
        cdb.close()
    except Exception:
        log.exception("audiences")
    return out


@app.get("/api/stocks/{company_id}")
async def stock(company_id: int, player=Depends(optional_player)):
    c = db()
    co = c.execute("SELECT * FROM companies WHERE id=?", (company_id,)).fetchone()
    if co is None:
        raise HTTPException(404, "没有这支股票")
    today = clock.today()
    t = c.execute("SELECT MIN(price) lo, MAX(price) hi, SUM(qty) v, SUM(qty*price) tv FROM trades WHERE company_id=? AND day=?",
                  (company_id, today)).fetchone()
    first = c.execute("SELECT price FROM trades WHERE company_id=? AND day=? ORDER BY id LIMIT 1", (company_id, today)).fetchone()
    now_ts = clock.ts()
    band = ex.cooling(co, now_ts)
    book = ex.book(c, company_id)
    auction = None
    if phase() == "auction":
        ap, av = ex.auction_price(c, company_id, co["prev_close"])
        auction = {"price": yuan(ap), "volume": av}
    trades = [{"price": yuan(r["price"]), "qty": r["qty"], "ts": r["ts"]} for r in c.execute(
        "SELECT price, qty, ts FROM trades WHERE company_id=? ORDER BY id DESC LIMIT 30", (company_id,))]
    weeks = []
    for r in c.execute("""SELECT strftime('%Y-%W', day) wk, MIN(day) start, SUM(revenue) rev, SUM(rev_us) us, SUM(rev_cn) cn,
                          SUM(rev_jp) jp, SUM(dividend_ps) dps, MAX(aired) aired
                          FROM company_days WHERE company_id=? GROUP BY wk ORDER BY wk""", (company_id,)):
        weeks.append({"week": r["start"], "revenue": yuan(r["rev"]), "us": yuan(r["us"]), "cn": yuan(r["cn"]),
                      "jp": yuan(r["jp"]), "dividend_ps": yuan(r["dps"]), "aired": r["aired"]})
    holders = c.execute("""SELECT a.kind, SUM(h.qty) q FROM holdings h JOIN accounts a ON a.id=h.account_id
                           WHERE h.company_id=? GROUP BY a.kind""", (company_id,)).fetchall()
    company_cash = c.execute("SELECT cash FROM accounts WHERE kind='company' AND company_id=?", (company_id,)).fetchone()
    ipo_open = c.execute("""SELECT o.qty - o.filled AS left FROM orders o JOIN accounts a ON a.id=o.account_id
                            WHERE o.company_id=? AND o.status='open' AND a.kind='company'""", (company_id,)).fetchone()
    out = {
        "id": co["id"], "code": co["code"], "title": co["title"], "title_ja": co["title_ja"], "title_en": co["title_en"], "studio": co["studio"],
        "status": co["status"], "episodes": co["episodes"], "long_runner": bool(co["long_runner"]),
        "first_air": co["first_air"], "listed_on": co["listed_on"], "shares": co["shares"],
        "ipo_price": yuan(co["ipo_price"]), "ipo_left": ipo_open["left"] if ipo_open else 0,
        "quote": {"price": yuan(co["last_price"]), "prev_close": yuan(co["prev_close"]),
                  "open": yuan(first["price"]) if first else None, "high": yuan(t["hi"]), "low": yuan(t["lo"]),
                  "volume": t["v"] or 0, "turnover": yuan(t["tv"] or 0), "open_ref": yuan(co["open_ref"])},
        "vcm": {"active": band is not None, "until": co["vcm_until"] if band else None,
                "low": yuan(band[0]) if band else None, "high": yuan(band[1]) if band else None,
                "count": co["vcm_count"]},
        "auction": auction,
        "book": {"bids": [{**b, "price": yuan(b["price"])} for b in book["bids"]],
                 "asks": [{**a, "price": yuan(a["price"])} for a in book["asks"]]},
        "trades": trades, "weeks": weeks,
        "book_value_ps": yuan(round(company_cash["cash"] / co["shares"])) if company_cash else None,
        "holders": {r["kind"]: r["q"] for r in holders},
        "audiences": _audiences(company_id),
        "news": [dict(r) for r in c.execute("SELECT id, day, kind, title, body FROM news WHERE company_id=? ORDER BY id DESC LIMIT 20",
                                            (company_id,))],
        "ipo": ipo_json(co, player["id"] if player else None) if co["sub_start"] else None,
    }
    if player:
        pos = next((p for p in portfolio_rows(player["id"]) if p["company_id"] == company_id), None)
        out["position"] = pos
        out["my_orders"] = [order_json(r) for r in c.execute(
            "SELECT * FROM orders WHERE account_id=? AND company_id=? AND status='open' ORDER BY id DESC", (player["id"], company_id))]
        out["available_cash"] = yuan(ex.available_cash(c, player["id"]))
    return out


@app.get("/api/stocks/{company_id}/candles")
async def candles(company_id: int):
    c = db()
    rows = [{"time": r["day"], "open": yuan(r["open"]), "high": yuan(r["high"]), "low": yuan(r["low"]),
             "close": yuan(r["close"]), "volume": r["volume"], "replay": r["source"] == "replay"}
            for r in c.execute("SELECT * FROM candles WHERE company_id=? ORDER BY day", (company_id,))]
    today = clock.today()
    t = c.execute("""SELECT MIN(price) lo, MAX(price) hi, SUM(qty) v,
                     (SELECT price FROM trades WHERE company_id=? AND day=? ORDER BY id LIMIT 1) op,
                     (SELECT price FROM trades WHERE company_id=? AND day=? ORDER BY id DESC LIMIT 1) cl
                     FROM trades WHERE company_id=? AND day=?""",
                  (company_id, today, company_id, today, company_id, today)).fetchone()
    if t["v"] and (not rows or rows[-1]["time"] < today):
        rows.append({"time": today, "open": yuan(t["op"]), "high": yuan(t["hi"]), "low": yuan(t["lo"]),
                     "close": yuan(t["cl"]), "volume": t["v"], "replay": False, "live": True})
    return {"candles": rows}


@app.get("/api/stocks/{company_id}/intraday")
async def intraday(company_id: int):
    rows = db().execute("""SELECT substr(ts, 1, 16) minute, SUM(qty) v,
                           (SELECT t2.price FROM trades t2 WHERE t2.company_id=t.company_id AND substr(t2.ts,1,16)=substr(t.ts,1,16)
                            ORDER BY t2.id DESC LIMIT 1) last
                           FROM trades t WHERE company_id=? AND day=? GROUP BY minute ORDER BY minute""",
                        (company_id, clock.today())).fetchall()
    return {"points": [{"minute": r["minute"], "price": yuan(r["last"]), "volume": r["v"]} for r in rows]}


# ---------------------------------------------------------------- orders

class OrderIn(BaseModel):
    company_id: int
    side: str
    kind: str = "limit"
    qty: int = Field(gt=0, le=10_000_000)
    price: float | str | None = None


def order_json(r) -> dict:
    return {"id": r["id"], "company_id": r["company_id"], "side": r["side"], "kind": r["kind"],
            "price": yuan(r["price"]), "qty": r["qty"], "filled": r["filled"], "status": r["status"],
            "created_at": r["created_at"], "updated_at": r["updated_at"]}


@app.post("/api/orders")
async def place(body: OrderIn, player=Depends(current_player)):
    ph = phase()
    if ph == "closed":
        raise HTTPException(400, f"现在休市（集合竞价 {cfg.game['auction_start']}，连续交易 "
                                 f"{cfg.game['trading_open']}–{cfg.game['trading_close']}）")
    price = to_cents(body.price) if body.kind == "limit" else None
    async with LOCK:
        c = db()
        co = c.execute("SELECT * FROM companies WHERE id=?", (body.company_id,)).fetchone()
        if co is None:
            raise HTTPException(404, "没有这支股票")
        c.execute("BEGIN IMMEDIATE")
        try:
            if ph == "continuous":
                ensure_opening_auction(c)
            res = ex.place_order(c, account_id=player["id"], company_id=body.company_id, side=body.side, kind=body.kind,
                                 qty=body.qty, price=price, day=clock.today(), ts=clock.ts(), fees=fees(), rules=RULES,
                                 match=(ph == "continuous"))
            c.execute("COMMIT")
        except Exception:
            c.execute("ROLLBACK")
            raise
    res["avg_price"] = yuan(res["avg_price"])
    return res


@app.delete("/api/orders/{order_id}")
async def cancel(order_id: int, player=Depends(current_player)):
    async with LOCK:
        c = db()
        c.execute("BEGIN IMMEDIATE")
        try:
            ex.cancel_order(c, account_id=player["id"], order_id=order_id, ts=clock.ts())
            c.execute("COMMIT")
        except Exception:
            c.execute("ROLLBACK")
            raise
    return {"ok": True}


@app.get("/api/orders")
async def my_orders(status: str = "open", player=Depends(current_player)):
    q = "SELECT o.*, c.title, c.code FROM orders o JOIN companies c ON c.id=o.company_id WHERE o.account_id=?"
    args: list = [player["id"]]
    if status == "open":
        q += " AND o.status='open'"
    q += " ORDER BY o.id DESC LIMIT 200"
    return {"orders": [{**order_json(r), "title": r["title"], "code": r["code"]} for r in db().execute(q, args)]}


@app.get("/api/trades")
async def my_trades(player=Depends(current_player)):
    rows = db().execute("""SELECT t.*, c.title, c.code FROM trades t JOIN companies c ON c.id=t.company_id
                           WHERE t.buyer_id=? OR t.seller_id=? ORDER BY t.id DESC LIMIT 200""",
                        (player["id"], player["id"])).fetchall()
    return {"trades": [{"id": r["id"], "company_id": r["company_id"], "code": r["code"], "title": r["title"],
                        "side": "buy" if r["buyer_id"] == player["id"] else "sell", "price": yuan(r["price"]),
                        "qty": r["qty"], "ts": r["ts"]} for r in rows]}


@app.get("/api/ledger")
async def my_ledger(player=Depends(current_player)):
    rows = db().execute("""SELECT l.*, c.title FROM ledger l LEFT JOIN companies c ON c.id=l.company_id
                           WHERE (l.to_id=? OR l.from_id=?) AND l.reason != 'trade' ORDER BY l.id DESC LIMIT 200""",
                        (player["id"], player["id"])).fetchall()
    return {"entries": [{"ts": r["ts"], "reason": r["reason"], "title": r["title"],
                         "amount": yuan(r["amount"] if r["to_id"] == player["id"] else -r["amount"])} for r in rows]}


# ---------------------------------------------------------------- IPO subscription (公开认购)

def ipo_json(co, player_id: int | None) -> dict:
    c = db()
    float_qty = round(co["shares"] * cfg.params["company"]["ipo_float"])
    base = round(float_qty * cfg.game["ipo_public_base"])
    agg = c.execute("SELECT COALESCE(SUM(qty),0) q, COUNT(*) n, COALESCE(SUM(allotted),0) a FROM ipo_applications WHERE company_id=?",
                    (co["id"],)).fetchone()
    mine = c.execute("SELECT qty, allotted FROM ipo_applications WHERE company_id=? AND account_id=?",
                     (co["id"], player_id)).fetchone() if player_id else None
    allotted = co["status"] != "subscribing"
    over = agg["q"] / base if base else 0.0
    return {
        "id": co["id"], "code": co["code"], "title": co["title"], "title_ja": co["title_ja"],
        "ipo_price": yuan(co["ipo_price"]), "sub_start": co["sub_start"], "sub_end": co["sub_end"],
        "listed_on": co["listed_on"], "first_air": co["first_air"], "float_qty": float_qty, "base_public_qty": base,
        "applied_qty": agg["q"], "applicants": agg["n"], "status": "allotted" if allotted else "open",
        "my": {"qty": mine["qty"], "allotted": mine["allotted"]} if mine else None,
        "result": {"oversubscribed": over, "public_qty": agg["a"], "ratio": agg["a"] / agg["q"] if agg["q"] else None}
                  if allotted else None,
    }


@app.get("/api/ipos")
async def ipos(player=Depends(optional_player)):
    """Open subscriptions, plus allotments of the last two weeks."""
    since = (date.fromisoformat(clock.today()) - timedelta(days=14)).isoformat()
    rows = db().execute("""SELECT * FROM companies WHERE sub_start IS NOT NULL AND (status='subscribing' OR sub_end >= ?)
                           ORDER BY status='subscribing' DESC, sub_end DESC""", (since,)).fetchall()
    return {"ipos": [ipo_json(r, player["id"] if player else None) for r in rows]}


class SubscribeIn(BaseModel):
    qty: int = Field(ge=0, le=10_000_000)


def _withdraw(c, company_id: int, account_id: int) -> None:
    old = c.execute("SELECT frozen FROM ipo_applications WHERE company_id=? AND account_id=?", (company_id, account_id)).fetchone()
    if old:
        c.execute("UPDATE accounts SET cash_frozen = cash_frozen - ? WHERE id=?", (old["frozen"], account_id))
        c.execute("DELETE FROM ipo_applications WHERE company_id=? AND account_id=?", (company_id, account_id))


@app.post("/api/ipos/{company_id}/subscribe")
async def subscribe(company_id: int, body: SubscribeIn, player=Depends(current_player)):
    async with LOCK:
        c = db()
        co = c.execute("SELECT * FROM companies WHERE id=?", (company_id,)).fetchone()
        if co is None or co["status"] != "subscribing":
            raise HTTPException(400, "这支新股不在认购期")
        today = clock.today()
        if not (co["sub_start"] <= today <= co["sub_end"]):
            raise HTTPException(400, f"认购时间是 {co['sub_start']} 至 {co['sub_end']}")
        c.execute("BEGIN IMMEDIATE")
        try:
            _withdraw(c, company_id, player["id"])
            frozen = 0
            if body.qty > 0:
                frozen = fees().buy_reserve(co["ipo_price"], body.qty)
                if ex.available_cash(c, player["id"]) < frozen:
                    raise HTTPException(400, f"可用资金不足：认购 {body.qty:,} 股需要冻结 {frozen / 100:,.2f} 币")
                c.execute("UPDATE accounts SET cash_frozen = cash_frozen + ? WHERE id=?", (frozen, player["id"]))
                c.execute("INSERT INTO ipo_applications(company_id, account_id, qty, frozen, created_at) VALUES (?,?,?,?,?)",
                          (company_id, player["id"], body.qty, frozen, clock.ts()))
            c.execute("COMMIT")
        except Exception:
            c.execute("ROLLBACK")
            raise
    return {"ok": True, "qty": body.qty, "frozen": yuan(frozen)}


@app.delete("/api/ipos/{company_id}/subscribe")
async def unsubscribe(company_id: int, player=Depends(current_player)):
    async with LOCK:
        c = db()
        co = c.execute("SELECT status FROM companies WHERE id=?", (company_id,)).fetchone()
        if co is None or co["status"] != "subscribing":
            raise HTTPException(400, "认购已经截止，不能撤回")
        c.execute("BEGIN IMMEDIATE")
        _withdraw(c, company_id, player["id"])
        c.execute("COMMIT")
    return {"ok": True}


# ---------------------------------------------------------------- community

@app.get("/api/leaderboard")
async def leaderboard():
    c = db()
    rows = []
    for a in c.execute("SELECT id, username, cash FROM accounts WHERE kind='player'").fetchall():
        stock = c.execute("""SELECT COALESCE(SUM(h.qty * co.last_price),0) FROM holdings h
                             JOIN companies co ON co.id=h.company_id WHERE h.account_id=?""", (a["id"],)).fetchone()[0]
        total = a["cash"] + stock
        rows.append({"username": a["username"], "total": yuan(total),
                     "return_pct": (total / 100 - cfg.game["initial_cash"]) / cfg.game["initial_cash"]})
    rows.sort(key=lambda r: -r["total"])
    for i, r in enumerate(rows):
        r["rank"] = i + 1
    return {"players": rows}


@app.get("/api/news")
async def all_news(limit: int = 50):
    return {"news": [dict(r) for r in db().execute(
        "SELECT n.id, n.day, n.ts, n.kind, n.title, n.body, n.company_id, c.code FROM news n LEFT JOIN companies c ON c.id=n.company_id "
        "ORDER BY n.id DESC LIMIT ?", (min(limit, 200),))]}


@app.get("/api/macro")
async def macro():
    rows = db().execute("SELECT * FROM macro_days ORDER BY day").fetchall()
    return {"days": [{"day": r["day"], "rate": r["rate"], "m2": yuan(r["m2"]), "m2_growth": r["m2_growth"],
                      "injected": yuan(r["injected"]), "exported": yuan(r["exported"]), "taxes": yuan(r["taxes"])}
                     for r in rows]}


# ---------------------------------------------------------------- static frontend

DIST = ROOT / "web" / "dist"
if DIST.exists():
    app.mount("/assets", StaticFiles(directory=DIST / "assets"), name="assets")

    @app.get("/{path:path}")
    async def spa(path: str):
        if path.startswith("api/"):
            raise HTTPException(404, "没有这个接口")
        f = DIST / path
        if path and f.is_file() and DIST in f.resolve().parents:
            return FileResponse(f)
        return FileResponse(DIST / "index.html")
