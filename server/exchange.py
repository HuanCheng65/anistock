"""Order book, matching and settlement.

Continuous double auction with price-time priority; trades execute at the resting order's price.
Limit orders may rest and fill partially; market orders sweep the book up to the day's price limit
and cancel any remainder (A-share "即时成交剩余撤销"). Cash for buys and shares for sells are frozen
when the order is placed, so nobody can spend the same money twice.

All functions take an open connection and run inside the caller's transaction.
"""

from __future__ import annotations

import math
import sqlite3
from dataclasses import dataclass
from datetime import datetime, timedelta

from . import db as gdb


class OrderError(Exception):
    """A rejected order. The message is shown to the player as-is (Chinese)."""


@dataclass
class Fees:
    commission: float = 0.00025   # each side, per fill
    stamp_duty: float = 0.0005    # seller only

    def commission_on(self, notional: int) -> int:
        return math.ceil(notional * self.commission) if notional > 0 else 0

    def stamp_on(self, notional: int) -> int:
        return math.ceil(notional * self.stamp_duty) if notional > 0 else 0

    def buy_reserve(self, price: int, qty: int) -> int:
        """Cash to freeze for a buy: notional + commission, plus one cent per share for per-fill rounding."""
        notional = price * qty
        return notional + self.commission_on(notional) + qty


# ---------------------------------------------------------------- cash and shares

def transfer(conn, from_id: int, to_id: int, amount: int, reason: str, *, day: str, ts: str,
             company_id: int | None = None, ref: int | None = None) -> None:
    if amount <= 0:
        return
    conn.execute("UPDATE accounts SET cash = cash - ? WHERE id = ?", (amount, from_id))
    conn.execute("UPDATE accounts SET cash = cash + ? WHERE id = ?", (amount, to_id))
    conn.execute("INSERT INTO ledger(ts, day, from_id, to_id, amount, reason, company_id, ref) VALUES (?,?,?,?,?,?,?,?)",
                 (ts, day, from_id, to_id, amount, reason, company_id, ref))


def add_shares(conn, account_id: int, company_id: int, qty: int, cost: int, day: str) -> None:
    if qty <= 0:
        return
    conn.execute("""INSERT INTO holdings(account_id, company_id, qty) VALUES (?,?,?)
                    ON CONFLICT(account_id, company_id) DO UPDATE SET qty = qty + excluded.qty""",
                 (account_id, company_id, qty))
    conn.execute("INSERT INTO lots(account_id, company_id, qty, cost, acquired_on) VALUES (?,?,?,?,?)",
                 (account_id, company_id, qty, cost, day))


def remove_shares(conn, account_id: int, company_id: int, qty: int, *, frozen: int = 0) -> list[tuple[int, int, str]]:
    """Take `qty` shares out FIFO; `frozen` of them were reserved by a sell order. Returns consumed lots."""
    conn.execute("UPDATE holdings SET qty = qty - ?, qty_frozen = qty_frozen - ? WHERE account_id=? AND company_id=?",
                 (qty, frozen, account_id, company_id))
    taken, left = [], qty
    for lot in conn.execute("SELECT id, qty, cost, acquired_on FROM lots WHERE account_id=? AND company_id=? ORDER BY id",
                            (account_id, company_id)).fetchall():
        if left == 0:
            break
        q = min(left, lot["qty"])
        if q == lot["qty"]:
            conn.execute("DELETE FROM lots WHERE id=?", (lot["id"],))
        else:
            conn.execute("UPDATE lots SET qty = qty - ? WHERE id=?", (q, lot["id"]))
        taken.append((q, lot["cost"], lot["acquired_on"]))
        left -= q
    if left:
        raise RuntimeError(f"lots short by {left} for account {account_id} company {company_id}")
    conn.execute("DELETE FROM holdings WHERE account_id=? AND company_id=? AND qty=0 AND qty_frozen=0",
                 (account_id, company_id))
    return taken


def available_cash(conn, account_id: int) -> int:
    r = conn.execute("SELECT cash - cash_frozen FROM accounts WHERE id=?", (account_id,)).fetchone()
    return r[0] if r else 0


def available_shares(conn, account_id: int, company_id: int) -> int:
    r = conn.execute("SELECT qty - qty_frozen FROM holdings WHERE account_id=? AND company_id=?",
                     (account_id, company_id)).fetchone()
    return r[0] if r else 0


# ---------------------------------------------------------------- volatility control (no daily limit)

@dataclass
class Rules:
    """Hong Kong-style volatility control mechanism (市场波动调节机制) instead of daily price limits.

    The first time today a trade would print more than `vcm_threshold` away from the stock's last
    price `vcm_window_min` minutes ago, it does not execute; the stock cools off for `vcm_cool_min`
    minutes and may only trade inside reference ± threshold. After that the anchor is today's opening
    price and the threshold widens by one step per trigger (±20%, ±30%...), so the brake keeps working
    across the day without being a hard limit.
    """
    vcm_threshold: float = 0.10
    vcm_window_min: int = 5
    vcm_cool_min: int = 5
    sanity_mult: int = 20          # a limit price may not exceed 20x the previous close


def _dt(ts: str) -> datetime:
    return datetime.fromisoformat(ts)


def cooling(company, ts: str) -> tuple[int, int] | None:
    """The cooling-off band if the stock is cooling off at `ts`."""
    if company["vcm_until"] and ts < company["vcm_until"]:
        return company["vcm_low"], company["vcm_high"]
    return None


def sanity_band(company, rules: Rules) -> tuple[int, int]:
    return 1, max(company["prev_close"] * rules.sanity_mult, 100)


def quote_band(company, ts: str, rules: Rules) -> tuple[int, int]:
    """Prices that can trade right now: the cooling band, else the sanity band."""
    return cooling(company, ts) or sanity_band(company, rules)


def _vcm_reference(conn, company, day: str, ts: str, rules: Rules) -> tuple[int, float]:
    anchor = company["open_ref"] or company["prev_close"]
    if company["vcm_count"] == 0:
        before = (_dt(ts) - timedelta(minutes=rules.vcm_window_min)).isoformat(timespec="seconds")
        r = conn.execute("""SELECT price FROM trades WHERE company_id=? AND day=? AND ts <= ?
                            ORDER BY id DESC LIMIT 1""", (company["id"], day, before)).fetchone()
        return (r["price"] if r else anchor), rules.vcm_threshold
    return anchor, rules.vcm_threshold * (company["vcm_count"] + 1)


def allow_fill(conn, company_id: int, tp: int, *, day: str, ts: str, rules: Rules) -> bool:
    """May a trade print at `tp` now? Starts a cooling-off period when it may not."""
    c = conn.execute("SELECT * FROM companies WHERE id=?", (company_id,)).fetchone()
    band = cooling(c, ts)
    if band:
        return band[0] <= tp <= band[1]
    ref, thr = _vcm_reference(conn, c, day, ts, rules)
    if ref <= 0 or abs(tp / ref - 1) <= thr + 1e-12:
        return True
    lo = max(1, math.ceil(ref * (1 - thr) - 1e-9))
    hi = math.floor(ref * (1 + thr) + 1e-9)
    until = (_dt(ts) + timedelta(minutes=rules.vcm_cool_min)).isoformat(timespec="seconds")
    conn.execute("UPDATE companies SET vcm_until=?, vcm_low=?, vcm_high=?, vcm_count=vcm_count+1 WHERE id=?",
                 (until, lo, hi, company_id))
    conn.execute("INSERT INTO news(ts, day, kind, company_id, title, body) VALUES (?,?, 'vcm', ?, ?, ?)",
                 (ts, day, company_id, f"《{c['title']}》触发冷静期",
                  f"成交价将偏离参考价 {ref / 100:.2f} 超过 {thr:.0%}，{rules.vcm_cool_min} 分钟内只能在 "
                  f"{lo / 100:.2f} – {hi / 100:.2f} 之间成交。"))
    return lo <= tp <= hi


# ---------------------------------------------------------------- orders

def place_order(conn, *, account_id: int, company_id: int, side: str, kind: str, qty: int,
                price: int | None, day: str, ts: str, fees: Fees, rules: Rules, match: bool = True) -> dict:
    """Validate, freeze, match. Returns {"order_id", "status", "filled", "avg_price"}.
    With match=False (the opening auction) a limit order only rests on the book."""
    if side not in ("buy", "sell"):
        raise OrderError("买卖方向不对")
    if kind not in ("limit", "market"):
        raise OrderError("委托类型不对")
    if not isinstance(qty, int) or qty <= 0:
        raise OrderError("数量必须是正整数")
    company = conn.execute("SELECT * FROM companies WHERE id=?", (company_id,)).fetchone()
    if company is None or company["status"] != "listed":
        raise OrderError("这支股票不存在或已退市")
    lo, hi = sanity_band(company, rules)
    if kind == "limit":
        if price is None or price <= 0:
            raise OrderError("限价单需要价格")
        if price > hi:
            raise OrderError(f"价格太离谱了：不能超过昨收的 {rules.sanity_mult} 倍（{hi / 100:.2f}）")
    else:
        if not match:
            raise OrderError("集合竞价阶段只接受限价单")
        price = None

    # freeze
    frozen = 0
    if side == "buy":
        if kind == "limit":
            frozen = fees.buy_reserve(price, qty)
            if available_cash(conn, account_id) < frozen:
                raise OrderError("可用资金不足")
            conn.execute("UPDATE accounts SET cash_frozen = cash_frozen + ? WHERE id=?", (frozen, account_id))
    else:
        if available_shares(conn, account_id, company_id) < qty:
            raise OrderError("可卖股数不足")
        conn.execute("UPDATE holdings SET qty_frozen = qty_frozen + ? WHERE account_id=? AND company_id=?",
                     (qty, account_id, company_id))

    cur = conn.execute("""INSERT INTO orders(account_id, company_id, side, kind, price, qty, frozen, status, created_at, updated_at)
                          VALUES (?,?,?,?,?,?,?, 'open', ?, ?)""",
                       (account_id, company_id, side, kind, price, qty, frozen, ts, ts))
    order_id = cur.lastrowid
    filled, notional = _match(conn, order_id, day=day, ts=ts, fees=fees, rules=rules) if match else (0, 0)

    order = conn.execute("SELECT * FROM orders WHERE id=?", (order_id,)).fetchone()
    if order["filled"] == order["qty"]:
        _close(conn, order, "filled", ts)
    elif kind == "market":
        _close(conn, order, "cancelled", ts)   # immediate-or-cancel remainder
    order = conn.execute("SELECT status FROM orders WHERE id=?", (order_id,)).fetchone()
    return {"order_id": order_id, "status": order["status"], "filled": filled,
            "avg_price": round(notional / filled) if filled else None}


def cancel_order(conn, *, account_id: int, order_id: int, ts: str) -> None:
    order = conn.execute("SELECT * FROM orders WHERE id=? AND account_id=?", (order_id, account_id)).fetchone()
    if order is None:
        raise OrderError("找不到这笔委托")
    if order["status"] != "open":
        raise OrderError("这笔委托已经结束，不能撤单")
    _close(conn, order, "cancelled", ts)


def _close(conn, order, status: str, ts: str) -> None:
    """End an order and release whatever it still has frozen."""
    if order["side"] == "buy" and order["frozen"] > 0:
        conn.execute("UPDATE accounts SET cash_frozen = cash_frozen - ? WHERE id=?", (order["frozen"], order["account_id"]))
    if order["side"] == "sell":
        left = order["qty"] - order["filled"]
        if left > 0:
            conn.execute("UPDATE holdings SET qty_frozen = qty_frozen - ? WHERE account_id=? AND company_id=?",
                         (left, order["account_id"], order["company_id"]))
            conn.execute("DELETE FROM holdings WHERE account_id=? AND company_id=? AND qty=0 AND qty_frozen=0",
                         (order["account_id"], order["company_id"]))
    conn.execute("UPDATE orders SET status=?, frozen=0, updated_at=? WHERE id=?", (status, ts, order["id"]))


def _match(conn, order_id: int, *, day: str, ts: str, fees: Fees, rules: Rules) -> tuple[int, int]:
    """Match an incoming order against the book. Returns (shares filled, notional). Stops at the first
    price the volatility control will not let print (the book is sorted, so later prices are worse)."""
    o = conn.execute("SELECT * FROM orders WHERE id=?", (order_id,)).fetchone()
    company = conn.execute("SELECT * FROM companies WHERE id=?", (o["company_id"],)).fetchone()
    lo, hi = sanity_band(company, rules)
    if o["side"] == "buy":
        limit = o["price"] if o["kind"] == "limit" else hi
        resting = conn.execute("""SELECT * FROM orders WHERE company_id=? AND status='open' AND side='sell'
                                  AND price <= ? AND id != ? ORDER BY price ASC, id ASC""",
                               (o["company_id"], limit, order_id)).fetchall()
    else:
        limit = o["price"] if o["kind"] == "limit" else lo
        resting = conn.execute("""SELECT * FROM orders WHERE company_id=? AND status='open' AND side='buy'
                                  AND price >= ? AND id != ? ORDER BY price DESC, id ASC""",
                               (o["company_id"], limit, order_id)).fetchall()

    remaining = o["qty"] - o["filled"]
    filled = notional = 0
    for r in resting:
        if remaining <= 0:
            break
        if r["account_id"] == o["account_id"]:
            continue                       # no self-trades
        r = conn.execute("SELECT * FROM orders WHERE id=?", (r["id"],)).fetchone()
        if r["status"] != "open":
            continue
        tp = r["price"]
        if not allow_fill(conn, o["company_id"], tp, day=day, ts=ts, rules=rules):
            break
        q = min(remaining, r["qty"] - r["filled"])
        if o["side"] == "buy" and o["kind"] == "market":
            # pay as you go: never spend more than is free right now
            per = tp + math.ceil(tp * fees.commission) + 1
            q = min(q, available_cash(conn, o["account_id"]) // per)
            if q <= 0:
                break
        buy, sell = (o, r) if o["side"] == "buy" else (r, o)
        _fill(conn, buy, sell, q, tp, day=day, ts=ts, fees=fees)
        remaining -= q
        filled += q
        notional += q * tp
        o = conn.execute("SELECT * FROM orders WHERE id=?", (order_id,)).fetchone()
    return filled, notional


def _fill(conn, buy, sell, q: int, tp: int, *, day: str, ts: str, fees: Fees) -> None:
    company_id = buy["company_id"]
    notional = q * tp
    c_buy = fees.commission_on(notional)
    c_sell = fees.commission_on(notional)
    stamp = fees.stamp_on(notional)
    world = gdb.special(conn, "world")
    gov = gdb.special(conn, "gov")

    cur = conn.execute("""INSERT INTO trades(company_id, price, qty, buyer_id, seller_id, buy_order, sell_order, ts, day)
                          VALUES (?,?,?,?,?,?,?,?,?)""",
                       (company_id, tp, q, buy["account_id"], sell["account_id"], buy["id"], sell["id"], ts, day))
    trade_id = cur.lastrowid

    # buyer pays; a resting/limit buy pays out of its frozen cash
    spend = notional + c_buy
    if buy["frozen"] > 0:
        use = min(spend, buy["frozen"])
        conn.execute("UPDATE orders SET frozen = frozen - ? WHERE id=?", (use, buy["id"]))
        conn.execute("UPDATE accounts SET cash_frozen = cash_frozen - ? WHERE id=?", (use, buy["account_id"]))
    transfer(conn, buy["account_id"], sell["account_id"], notional, "trade", day=day, ts=ts, company_id=company_id, ref=trade_id)
    transfer(conn, buy["account_id"], world, c_buy, "commission", day=day, ts=ts, company_id=company_id, ref=trade_id)
    transfer(conn, sell["account_id"], world, c_sell, "commission", day=day, ts=ts, company_id=company_id, ref=trade_id)
    transfer(conn, sell["account_id"], gov, stamp, "stamp_duty", day=day, ts=ts, company_id=company_id, ref=trade_id)

    remove_shares(conn, sell["account_id"], company_id, q, frozen=q)
    add_shares(conn, buy["account_id"], company_id, q, tp, day)

    for o in (buy, sell):
        conn.execute("UPDATE orders SET filled = filled + ?, updated_at=? WHERE id=?", (q, ts, o["id"]))
        o2 = conn.execute("SELECT * FROM orders WHERE id=?", (o["id"],)).fetchone()
        if o2["filled"] == o2["qty"]:
            _close(conn, o2, "filled", ts)
    conn.execute("UPDATE companies SET last_price=? WHERE id=?", (tp, company_id))


def book(conn, company_id: int, depth: int = 5) -> dict:
    def side(s, order):
        rows = conn.execute(f"""SELECT price, SUM(qty - filled) AS qty, COUNT(*) AS n FROM orders
                                WHERE company_id=? AND status='open' AND side=? GROUP BY price
                                ORDER BY price {order} LIMIT ?""", (company_id, s, depth)).fetchall()
        return [{"price": r["price"], "qty": r["qty"], "orders": r["n"]} for r in rows]
    return {"bids": side("buy", "DESC"), "asks": side("sell", "ASC")}


# ---------------------------------------------------------------- opening auction (集合竞价)

def auction_price(conn, company_id: int, ref: int) -> tuple[int | None, int]:
    """Equilibrium price of the resting limit orders: most volume, then least imbalance, then closest
    to the reference (previous close). Returns (price, volume); (None, 0) if nothing crosses."""
    buys = conn.execute("""SELECT price, SUM(qty-filled) q FROM orders WHERE company_id=? AND status='open'
                           AND side='buy' AND kind='limit' GROUP BY price""", (company_id,)).fetchall()
    sells = conn.execute("""SELECT price, SUM(qty-filled) q FROM orders WHERE company_id=? AND status='open'
                            AND side='sell' AND kind='limit' GROUP BY price""", (company_id,)).fetchall()
    if not buys or not sells:
        return None, 0
    best = None
    for p in sorted({r["price"] for r in buys} | {r["price"] for r in sells}):
        demand = sum(r["q"] for r in buys if r["price"] >= p)
        supply = sum(r["q"] for r in sells if r["price"] <= p)
        vol = min(demand, supply)
        if vol <= 0:
            continue
        key = (vol, -abs(demand - supply), -abs(p - ref))
        if best is None or key > best[0]:
            best = (key, p, vol)
    return (best[1], best[2]) if best else (None, 0)


def run_auction(conn, company_id: int, *, day: str, ts: str, fees: Fees) -> int | None:
    """Clear the opening auction at one price. Returns the opening price, or None if nothing crossed."""
    c = conn.execute("SELECT * FROM companies WHERE id=?", (company_id,)).fetchone()
    p, vol = auction_price(conn, company_id, c["prev_close"])
    if p is None:
        return None
    buys = conn.execute("""SELECT id FROM orders WHERE company_id=? AND status='open' AND side='buy' AND kind='limit'
                           AND price >= ? ORDER BY price DESC, id ASC""", (company_id, p)).fetchall()
    sells = conn.execute("""SELECT id FROM orders WHERE company_id=? AND status='open' AND side='sell' AND kind='limit'
                            AND price <= ? ORDER BY price ASC, id ASC""", (company_id, p)).fetchall()
    left = vol
    for b in buys:
        if left <= 0:
            break
        for s_ in sells:
            if left <= 0:
                break
            buy = conn.execute("SELECT * FROM orders WHERE id=?", (b["id"],)).fetchone()
            sell = conn.execute("SELECT * FROM orders WHERE id=?", (s_["id"],)).fetchone()
            if buy["status"] != "open":
                break
            if sell["status"] != "open" or sell["account_id"] == buy["account_id"]:
                continue
            q = min(buy["qty"] - buy["filled"], sell["qty"] - sell["filled"], left)
            if q > 0:
                _fill(conn, buy, sell, q, p, day=day, ts=ts, fees=fees)
                left -= q
    return p if left < vol else None
