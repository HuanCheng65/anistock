"""NPC traders. They are ordinary accounts with finite cash and shares, and trade through the same book.

market maker (one per stock)
    Quotes both sides around its private reference price: the hidden fair value with a daily noise
    term, blended with the last trade price. Leans its quotes against its inventory, and stays out of
    a stock's IPO so new listings go to players.
rights holders (制作委员会)
    Sell a little whenever the price runs well above value (大股东减持).
noise traders
    Small random trades so the book moves.
"""

from __future__ import annotations

import hashlib
import math
import random

from . import exchange as ex


def _daily_noise(company_id: int, day: str, sigma: float) -> float:
    """Same draw all day for a given stock, so the market maker's view is stable within a day."""
    h = int(hashlib.sha256(f"{company_id}:{day}".encode()).hexdigest()[:12], 16)
    return random.Random(h).gauss(0, sigma)


def _cancel_all(conn, account_id: int, company_id: int, ts: str) -> None:
    for o in conn.execute("SELECT * FROM orders WHERE account_id=? AND company_id=? AND status='open'",
                          (account_id, company_id)).fetchall():
        ex._close(conn, o, "cancelled", ts)


def _place(conn, **kw) -> None:
    try:
        ex.place_order(conn, **kw)
    except ex.OrderError:
        pass


def tick(conn, cfg, day: str, ts: str, rng: random.Random) -> None:
    g, npc = cfg.game, cfg.npc
    fees = ex.Fees(commission=g["commission"], stamp_duty=g["stamp_duty"])
    companies = conn.execute("SELECT * FROM companies WHERE status='listed'").fetchall()
    float_frac = cfg.params["company"]["ipo_float"]
    for c in companies:
        band = ex.price_band(c, day, g["price_limit"], g["ipo_free_days"], g.get("airing_free_days", 0))
        lo, hi, free = band
        mm = conn.execute("SELECT * FROM accounts WHERE kind='mm' AND company_id=?", (c["id"],)).fetchone()
        if mm is None or c["fair_value"] <= 0:
            continue
        _cancel_all(conn, mm["id"], c["id"], ts)
        ipo_open = conn.execute("""SELECT 1 FROM orders o JOIN accounts a ON a.id=o.account_id
                                   WHERE o.company_id=? AND o.status='open' AND a.kind='company'""", (c["id"],)).fetchone()

        fair = c["fair_value"] * (1 + _daily_noise(c["id"], day, npc["mm_value_noise"]))
        a = npc["mm_anchor_to_price"]
        ref = (1 - a) * fair + a * c["last_price"]
        inv = ex.available_shares(conn, mm["id"], c["id"])
        target = c["shares"] * float_frac * 0.5
        # inventory lean: selling out raises its quotes, piling up lowers them (price impact of big orders)
        skew = max(-0.5, min(0.5, npc["mm_inventory_skew"] * (inv - target) / max(1.0, target)))
        half = max(npc["mm_spread"] / 2 * ref, 1)
        # the lean only moves one side: short of stock it raises the ask (big buyers pay up), long it
        # lowers the bid. It never bids above, or offers below, its own view, so it cannot be farmed.
        ask_px = ref + half if skew >= 0 else ref * (1 - skew) + half
        bid_px = ref - half if skew <= 0 else ref * (1 - skew) - half
        # clamp to the day's limits; when its view is beyond the limit it still trades at the limit price
        # (sellers at 涨停, buyers at 跌停), keeping the bid one tick under the ask
        ask = max(lo + 1, min(hi, math.ceil(ask_px)))
        bid = max(lo, min(ask - 1, math.floor(bid_px)))
        size = max(1, round(c["shares"] * npc["mm_quote_frac"]))

        if not ipo_open:
            cash = ex.available_cash(conn, mm["id"])
            q = min(size, cash // max(1, fees.buy_reserve(bid, 1)))
            if q > 0 and bid >= lo:
                _place(conn, account_id=mm["id"], company_id=c["id"], side="buy", kind="limit", qty=q, price=bid,
                       day=day, ts=ts, fees=fees, band=band)
        q = min(size, ex.available_shares(conn, mm["id"], c["id"]))
        if q > 0 and ask <= hi:
            _place(conn, account_id=mm["id"], company_id=c["id"], side="sell", kind="limit", qty=q, price=ask,
                   day=day, ts=ts, fees=fees, band=band)

        # rights holders trim when the price runs far above value
        com = conn.execute("SELECT * FROM accounts WHERE kind='committee' AND company_id=?", (c["id"],)).fetchone()
        if com:
            _cancel_all(conn, com["id"], c["id"], ts)
            last = conn.execute("SELECT last_price FROM companies WHERE id=?", (c["id"],)).fetchone()[0]
            if last > c["fair_value"] * npc["committee_sell_above"]:
                q = min(ex.available_shares(conn, com["id"], c["id"]),
                        max(1, round(c["shares"] * npc["committee_sell_frac"])))
                price = max(lo, min(hi, math.ceil(c["fair_value"] * npc["committee_sell_above"])))
                if q > 0:
                    _place(conn, account_id=com["id"], company_id=c["id"], side="sell", kind="limit", qty=q,
                           price=price, day=day, ts=ts, fees=fees, band=band)

    # noise traders
    listed = [c for c in companies if not conn.execute(
        "SELECT 1 FROM orders o JOIN accounts a ON a.id=o.account_id WHERE o.company_id=? AND o.status='open' AND a.kind='company'",
        (c["id"],)).fetchone()]
    if not listed:
        return
    for n in conn.execute("SELECT * FROM accounts WHERE kind='noise'").fetchall():
        if rng.random() > npc["noise_trade_prob"]:
            continue
        c = rng.choice(listed)
        band = ex.price_band(c, day, g["price_limit"], g["ipo_free_days"], g.get("airing_free_days", 0))
        held = ex.available_shares(conn, n["id"], c["id"])
        side = "sell" if held > 0 and rng.random() < 0.5 else "buy"
        last = conn.execute("SELECT last_price FROM companies WHERE id=?", (c["id"],)).fetchone()[0]
        if side == "buy":
            budget = ex.available_cash(conn, n["id"]) * rng.uniform(0.01, 0.05)
            qty = int(budget // max(1, last))
        else:
            qty = max(1, int(held * rng.uniform(0.2, 1.0)))
        if qty <= 0:
            continue
        _place(conn, account_id=n["id"], company_id=c["id"], side=side, kind="market", qty=qty, price=None,
               day=day, ts=ts, fees=fees, band=band)
