"""The daily close: everything the game world does once a day (06:30, after the collector).

For day D (yesterday):
  1. revenue: external demand split by each show's weighted share of the three markets
     (world -> company), operating costs (company -> world), corporate tax (company -> gov)
  2. valuation: each company's hidden fair value, used by the NPCs
  3. Sunday: dividends (founders' part leaves the economy, other holders pay a holding-period tax),
     transfers to players, the central bank's rate decision
  4. interest on deposits (cb -> holders)
  5. IPO production fees on yesterday's IPO proceeds, new listings for tomorrow, delistings
  6. candles, previous close, news, macro row; invariants checked at the end

Data the day needs but does not have (the collector missed a run) is carried forward by the
engine's gap handling; the step never crashes the clock for missing data.
"""

from __future__ import annotations

import json
import logging
import math
from datetime import date, timedelta
from pathlib import Path

from engine import economy as E

from . import db as gdb
from . import exchange as ex
from .config import ROOT, Config

log = logging.getLogger("daily")
MARKET_KINDS = ("player", "mm", "noise", "committee", "company")


# ---------------------------------------------------------------- inputs

def load_inputs(cfg: Config, conn, day: str):
    p = cfg.params
    start = date.fromisoformat(gdb.get_state(conn, "window_start"))
    shows, _ = E.load_shows(cfg.collector_db, p, start, date.fromisoformat(day))
    mpath = ROOT / "data" / "derived" / "markets.json"
    markets = {int(k): v for k, v in json.loads(mpath.read_text(encoding="utf-8"))["series"].items()} if mpath.exists() else {}
    aud = E.load_audiences(cfg.collector_db)
    return {s.id: s for s in shows}, markets, aud


def ex_fees(cfg: Config) -> ex.Fees:
    return ex.Fees(commission=cfg.game["commission"], stamp_duty=cfg.game["stamp_duty"])


def account_of(conn, kind: str, company_id: int) -> int:
    return conn.execute("SELECT id FROM accounts WHERE kind=? AND company_id=?", (kind, company_id)).fetchone()["id"]


def news(conn, day: str, ts: str, kind: str, title: str, body: str = "", company_id: int | None = None) -> None:
    conn.execute("INSERT INTO news(ts, day, kind, company_id, title, body) VALUES (?,?,?,?,?,?)",
                 (ts, day, kind, company_id, title, body))


# ---------------------------------------------------------------- valuation

def margin(p: dict) -> float:
    co = p["company"]
    return (1 - co.get("operating_cost_ratio", 0)) * (1 - co["corp_tax"]) * co["payout_ratio"]


def expected_share(show, d: date, listed_shows, p, markets, aud) -> float:
    """This show's expected share of external demand, with every listed show's expectation in the denominator."""
    rv, val, w = p["revenue"], p["valuation"], p["markets"]
    tot = {m: 0.0 for m in E.MARKETS}
    own = None
    for s in listed_shows:
        a = E.expected_att(s, d, rv, markets, val, aud)
        for m in E.MARKETS:
            tot[m] += a[m]
        if s.id == show.id:
            own = a
    if own is None:
        own = E.expected_att(show, d, rv, markets, val, aud)
        for m in E.MARKETS:
            tot[m] += own[m]
    share, _ = E.blended_share(own, tot, w)
    return share


def fair_value(conn, cfg: Config, show, d: date, company: dict, book_cents: int, rate: float,
               demand_week: float, pre_share: float) -> int:
    """Per-share value in cents: the simulation's valuation applied to the company's own revenue history."""
    p = cfg.params
    shares = company["shares"]
    since = (d - show.first_air).days + 1
    rows = conn.execute("SELECT revenue FROM company_days WHERE company_id=? AND day<=? ORDER BY day DESC LIMIT 7",
                        (company["id"], d.isoformat())).fetchall()
    revs = [r["revenue"] / 100 for r in rows]
    preair_weekly = demand_week * pre_share
    if since < 7:
        hist = revs[:max(1, since)]
        weekly_rev = sum(hist) + preair_weekly / 7 * (7 - len(hist))
    else:
        weekly_rev = sum(revs) * 7 / max(1, len(revs))
    weekly_div = weekly_rev * margin(p) / shares
    preair_div = preair_weekly * margin(p) / shares
    use_pre = not E.has_real_data(show, d, p["valuation"])
    v = E.fundamental(show, d, weekly_div, book_cents / 100 / shares, rate, p, preair_div, use_preair=use_pre)
    return max(1, round(v * 100))


# ---------------------------------------------------------------- the step

def run_day(conn, cfg: Config, day: str, ts: str, inputs=None) -> dict:
    """Close `day`. Idempotent per day via state 'last_closed'."""
    if gdb.get_state(conn, "last_closed") and gdb.get_state(conn, "last_closed") >= day:
        return {"skipped": True}
    p, g = cfg.params, cfg.game
    co_p, rv = p["company"], p["revenue"]
    d = date.fromisoformat(day)
    shows, markets, aud = inputs or load_inputs(cfg, conn, day)
    world, cb, gov = (gdb.special(conn, k) for k in ("world", "cb", "gov"))
    rate = float(gdb.get_state(conn, "rate"))
    a_ref = float(gdb.get_state(conn, "a_ref"))
    summary = {"day": day, "injected": 0, "exported": 0, "taxes": 0, "interest": 0}

    conn.execute("BEGIN IMMEDIATE")
    try:
        companies = [dict(r) for r in conn.execute("SELECT * FROM companies WHERE status='listed'").fetchall()]
        listed_shows = [shows[c["id"]] for c in companies if c["id"] in shows]

        # 1. revenue
        att = {s.id: E.market_att(s, d, rv, markets) for s in listed_shows}
        totals = {m: sum(a[m] for a in att.values()) for m in E.MARKETS}
        tot_us = totals["us"]
        demand_day = rv["demand_base_week"] / 7 * ((tot_us / a_ref) ** rv["demand_elasticity"] if tot_us > 0 else 0)
        for c in companies:
            s = shows.get(c["id"])
            if s is None:
                continue
            share, parts = E.blended_share(att[s.id], totals, p["markets"])
            rev = round(demand_day * share * 100)
            split = [round(demand_day * parts[m] * 100) for m in E.MARKETS]
            acct = account_of(conn, "company", c["id"])
            ex.transfer(conn, world, acct, rev, "revenue", day=day, ts=ts, company_id=c["id"])
            cost = round(rev * co_p.get("operating_cost_ratio", 0))
            ex.transfer(conn, acct, world, cost, "operating_cost", day=day, ts=ts, company_id=c["id"])
            tax = round((rev - cost) * co_p["corp_tax"])
            ex.transfer(conn, acct, gov, tax, "corp_tax", day=day, ts=ts, company_id=c["id"])
            conn.execute("UPDATE companies SET profit_week = profit_week + ? WHERE id=?", (rev - cost - tax, c["id"]))
            aired = E.at(s, s.aired, d, 0)
            prev = conn.execute("SELECT aired FROM company_days WHERE company_id=? ORDER BY day DESC LIMIT 1",
                                (c["id"],)).fetchone()
            conn.execute("""INSERT OR REPLACE INTO company_days(company_id, day, revenue, rev_us, rev_cn, rev_jp, aired)
                            VALUES (?,?,?,?,?,?,?)""", (c["id"], day, rev, *split, aired))
            summary["injected"] += rev
            summary["exported"] += cost
            summary["taxes"] += tax
            if prev and aired > prev["aired"]:
                if prev["aired"] == 0:
                    news(conn, day, ts, "premiere", f"《{c['title']}》开播", "第 1 集已播出，各市场的热度数据将陆续进入营收。", c["id"])
                elif c["episodes"] and aired >= c["episodes"]:
                    news(conn, day, ts, "finale", f"《{c['title']}》完结", "完结后营收通常会在几周内明显下降。", c["id"])

        # 2. Sunday: dividends, transfers, rate decision
        if d.weekday() == 6:
            pay_dividends(conn, cfg, day, ts, companies, summary)
            weekly_taxes = conn.execute("""SELECT COALESCE(SUM(amount),0) FROM ledger WHERE to_id=? AND day>? AND day<=?
                                           AND reason IN ('corp_tax','dividend_tax','stamp_duty')""",
                                        (gov, (d - timedelta(days=7)).isoformat(), day)).fetchone()[0]
            pay_transfers(conn, cfg, day, ts, gov, round(weekly_taxes * g["transfer_ratio"]))
            rate = rate_decision(conn, cfg, day, ts, rate)

        # 3. interest on deposits (created by the central bank)
        r_dep = max(0.0, rate - p["central_bank"]["deposit_spread"])
        if r_dep > 0:
            for a in conn.execute("SELECT id, cash FROM accounts WHERE kind IN ('player','mm','noise') AND cash > 0").fetchall():
                i = math.floor(a["cash"] * r_dep / 365)
                ex.transfer(conn, cb, a["id"], i, "interest", day=day, ts=ts)
                summary["interest"] += i

        # 4. production fees on IPO proceeds received today
        for c in companies:
            acct = account_of(conn, "company", c["id"])
            proceeds = conn.execute("SELECT COALESCE(SUM(price*qty),0) FROM trades WHERE seller_id=? AND day=?",
                                    (acct, day)).fetchone()[0]
            fee = round(proceeds * co_p["production_fee_ratio"])
            if fee:
                ex.transfer(conn, acct, world, fee, "production_fee", day=day, ts=ts, company_id=c["id"])
                summary["exported"] += fee

        # 5. valuations, delistings
        demand_week = rv["demand_base_week"] * ((tot_us / a_ref) ** rv["demand_elasticity"] if tot_us > 0 else 0)
        total_week_rev = conn.execute("SELECT COALESCE(SUM(revenue),0) FROM company_days WHERE day>?",
                                      ((d - timedelta(days=7)).isoformat(),)).fetchone()[0]
        for c in companies:
            s = shows.get(c["id"])
            if s is None:
                continue
            book = conn.execute("SELECT cash FROM accounts WHERE kind='company' AND company_id=?", (c["id"],)).fetchone()["cash"]
            pre = expected_share(s, d, listed_shows, p, markets, aud)
            fv = fair_value(conn, cfg, s, d, c, book, rate, demand_week, pre)
            conn.execute("UPDATE companies SET fair_value=? WHERE id=?", (fv, c["id"]))
            if d.weekday() == 6:
                maybe_delist(conn, cfg, day, ts, c, s, total_week_rev, summary)

        # 6. orders are good for the day only (当日有效); the IPO offer stays on the book
        for o in conn.execute("""SELECT o.* FROM orders o JOIN accounts a ON a.id = o.account_id
                                 WHERE o.status='open' AND a.kind != 'company'""").fetchall():
            ex._close(conn, o, "cancelled", ts)

        # 7. candles and next day's reference price
        for c in conn.execute("SELECT * FROM companies WHERE status='listed'").fetchall():
            close_candle(conn, c, day)

        # 8. new listings for tomorrow
        list_new(conn, cfg, day, ts, shows, listed_shows, markets, aud, rate, demand_week)

        m2 = conn.execute(f"SELECT COALESCE(SUM(cash),0) FROM accounts WHERE kind IN {MARKET_KINDS}").fetchone()[0]
        old = conn.execute("SELECT m2 FROM macro_days WHERE day=?", ((d - timedelta(days=28)).isoformat(),)).fetchone()
        growth = (m2 / old["m2"]) ** (365 / 28) - 1 if old and old["m2"] > 0 else None
        conn.execute("""INSERT OR REPLACE INTO macro_days(day, rate, m2, m2_growth, injected, exported, taxes, interest)
                        VALUES (?,?,?,?,?,?,?,?)""",
                     (day, rate, m2, growth, summary["injected"], summary["exported"], summary["taxes"], summary["interest"]))
        gdb.set_state(conn, "rate", rate)
        gdb.set_state(conn, "last_closed", day)
        gdb.check_invariants(conn)
        conn.execute("COMMIT")
    except Exception:
        conn.execute("ROLLBACK")
        raise
    summary["rate"] = rate
    return summary


def holding_tax_rate(cfg: Config, acquired_on: str, day: str) -> float:
    days = (date.fromisoformat(day) - date.fromisoformat(acquired_on)).days
    for max_days, r in cfg.game["dividend_tax"]:
        if days <= max_days:
            return r
    return 0.0


def pay_dividends(conn, cfg: Config, day: str, ts: str, companies: list[dict], summary: dict) -> None:
    world, gov = gdb.special(conn, "world"), gdb.special(conn, "gov")
    payout = cfg.params["company"]["payout_ratio"]
    for c in companies:
        acct = account_of(conn, "company", c["id"])
        cash = conn.execute("SELECT cash FROM accounts WHERE id=?", (acct,)).fetchone()["cash"]
        pool = min(cash, round(max(0, c["profit_week"]) * payout))
        dps = pool // c["shares"]
        if dps <= 0:
            continue
        paid = 0
        for h in conn.execute("""SELECT h.account_id, h.qty, a.kind FROM holdings h JOIN accounts a ON a.id = h.account_id
                                 WHERE h.company_id=? AND h.qty > 0""", (c["id"],)).fetchall():
            if h["account_id"] == acct:
                continue                                   # unsold IPO shares held by the company itself
            gross = dps * h["qty"]
            if h["kind"] == "committee":
                # rights holders are real-world companies: their dividends leave the economy
                ex.transfer(conn, acct, world, gross, "dividend_export", day=day, ts=ts, company_id=c["id"])
                summary["exported"] += gross
            else:
                tax = 0
                for lot in conn.execute("SELECT qty, acquired_on FROM lots WHERE account_id=? AND company_id=?",
                                        (h["account_id"], c["id"])).fetchall():
                    tax += math.floor(dps * lot["qty"] * holding_tax_rate(cfg, lot["acquired_on"], day))
                ex.transfer(conn, acct, h["account_id"], gross - tax, "dividend", day=day, ts=ts, company_id=c["id"])
                ex.transfer(conn, acct, gov, tax, "dividend_tax", day=day, ts=ts, company_id=c["id"])
                summary["taxes"] += tax
            paid += gross
        conn.execute("UPDATE companies SET profit_week = 0 WHERE id=?", (c["id"],))
        conn.execute("INSERT OR IGNORE INTO company_days(company_id, day, revenue, rev_us, rev_cn, rev_jp, aired) VALUES (?,?,0,0,0,0,0)",
                     (c["id"], day))
        conn.execute("UPDATE company_days SET dividend_ps=? WHERE company_id=? AND day=?", (dps, c["id"], day))
        news(conn, day, ts, "dividend", f"《{c['title']}》每股分红 {dps / 100:.2f} 币",
             f"共派发 {paid / 100:,.0f} 币。持有不满 1 周的部分按 20% 缴税，不满 1 季按 10%，超过 1 季免税。", c["id"])


def pay_transfers(conn, cfg: Config, day: str, ts: str, gov: int, amount: int) -> None:
    players = [r["id"] for r in conn.execute("SELECT id FROM accounts WHERE kind='player'").fetchall()]
    if not players or amount <= 0:
        return
    have = conn.execute("SELECT cash FROM accounts WHERE id=?", (gov,)).fetchone()["cash"]
    each = min(amount, have) // len(players)
    for pid in players:
        ex.transfer(conn, gov, pid, each, "transfer", day=day, ts=ts)
    if each:
        news(conn, day, ts, "system", f"财政补贴：每位玩家 {each / 100:,.2f} 币", "本周税收的一部分按人头返还。")


def rate_decision(conn, cfg: Config, day: str, ts: str, rate: float) -> float:
    cb = cfg.params["central_bank"]
    L = cb["growth_lookback_days"]
    d = date.fromisoformat(day)
    now = conn.execute(f"SELECT COALESCE(SUM(cash),0) FROM accounts WHERE kind IN {MARKET_KINDS}").fetchone()[0]
    old = conn.execute("SELECT m2 FROM macro_days WHERE day=?", ((d - timedelta(days=L)).isoformat(),)).fetchone()
    if not old or old["m2"] <= 0:
        return rate
    g = (now / old["m2"]) ** (365 / L) - 1
    want = min(cb["r_max"], max(cb["r_min"], cb["r_neutral"] + cb["k_money"] * (g - cb["m2_growth_target"])))
    new = rate + max(-cb["max_step"], min(cb["max_step"], want - rate))
    new = round(new, 6)
    if abs(new - rate) > 1e-9:
        verb = "加息" if new > rate else "降息"
        news(conn, day, ts, "rate", f"央行{verb}：政策利率 {rate:.2%} → {new:.2%}",
             f"过去 {L} 天货币供应量年化增速 {g:.1%}，目标 {cb['m2_growth_target']:.0%}。存款利率随之变为 {max(0, new - cb['deposit_spread']):.2%}。")
    return new


def maybe_delist(conn, cfg: Config, day: str, ts: str, c: dict, s, total_week_rev: int, summary: dict) -> None:
    g = cfg.game
    d = date.fromisoformat(day)
    finished = (not s.long_runner) and s.episodes and E.at(s, s.aired, d, 0) >= s.episodes
    week_rev = conn.execute("SELECT COALESCE(SUM(revenue),0) FROM company_days WHERE company_id=? AND day>?",
                            (c["id"], (d - timedelta(days=7)).isoformat())).fetchone()[0]
    low = finished and week_rev < g["delist_revenue_share"] * max(1, total_week_rev)
    weeks = c["low_weeks"] + 1 if low else 0
    conn.execute("UPDATE companies SET low_weeks=? WHERE id=?", (weeks, c["id"]))
    if weeks < g["delist_after_weeks"]:
        return
    # cancel open orders, return company cash per share, retire the shares
    for o in conn.execute("SELECT * FROM orders WHERE company_id=? AND status='open'", (c["id"],)).fetchall():
        ex._close(conn, o, "cancelled", ts)
    acct = account_of(conn, "company", c["id"])
    world = gdb.special(conn, "world")
    cash = conn.execute("SELECT cash FROM accounts WHERE id=?", (acct,)).fetchone()["cash"]
    holders = conn.execute("""SELECT h.account_id, h.qty, a.kind FROM holdings h JOIN accounts a ON a.id=h.account_id
                              WHERE h.company_id=? AND h.qty>0""", (c["id"],)).fetchall()
    per = cash // c["shares"]
    for h in holders:
        amt = per * h["qty"]
        if h["account_id"] != acct:
            to = world if h["kind"] == "committee" else h["account_id"]
            ex.transfer(conn, acct, to, amt, "delisting", day=day, ts=ts, company_id=c["id"])
        ex.remove_shares(conn, h["account_id"], c["id"], h["qty"])
    left = conn.execute("SELECT cash FROM accounts WHERE id=?", (acct,)).fetchone()["cash"]
    ex.transfer(conn, acct, world, left, "delisting", day=day, ts=ts, company_id=c["id"])
    conn.execute("UPDATE companies SET status='delisted', delisted_on=? WHERE id=?", (day, c["id"]))
    news(conn, day, ts, "delisting", f"《{c['title']}》退市", f"按每股净资产 {per / 100:.2f} 币向股东返还现金。", c["id"])


def close_candle(conn, c, day: str) -> None:
    t = conn.execute("""SELECT MIN(price) lo, MAX(price) hi, SUM(qty) vol,
                        (SELECT price FROM trades WHERE company_id=? AND day=? ORDER BY id LIMIT 1) op,
                        (SELECT price FROM trades WHERE company_id=? AND day=? ORDER BY id DESC LIMIT 1) cl
                        FROM trades WHERE company_id=? AND day=?""",
                     (c["id"], day, c["id"], day, c["id"], day)).fetchone()
    if t["vol"]:
        o, h, lo, cl, v = t["op"], t["hi"], t["lo"], t["cl"], t["vol"]
    else:
        o = h = lo = cl = c["last_price"]
        v = 0
    conn.execute("""INSERT OR REPLACE INTO candles(company_id, day, open, high, low, close, volume, source)
                    VALUES (?,?,?,?,?,?,?, 'live')""", (c["id"], day, o, h, lo, cl, v))
    conn.execute("UPDATE companies SET prev_close=?, last_price=? WHERE id=?", (cl, cl, c["id"]))


def list_new(conn, cfg: Config, day: str, ts: str, shows: dict, listed_shows: list, markets, aud,
             rate: float, demand_week: float) -> None:
    """List shows whose premiere is within ipo_lead_days of tomorrow. The float goes on the book at the IPO
    price, first come first served; the company keeps unsold shares on offer."""
    p, g = cfg.params, cfg.game
    tomorrow = date.fromisoformat(day) + timedelta(days=1)
    lead = p["universe"]["ipo_lead_days"]
    existing = {r["id"] for r in conn.execute("SELECT id FROM companies").fetchall()}
    for s in shows.values():
        if s.id in existing or s.popularity_end < g["min_listing_popularity"]:
            continue
        if not (s.first_air - timedelta(days=lead) <= tomorrow):
            continue
        if s.first_air < tomorrow - timedelta(days=60) and not s.long_runner:
            continue                                   # long finished before the game noticed it
        create_company(conn, cfg, s, tomorrow.isoformat(), day, ts, listed_shows, markets, aud, rate, demand_week)


def create_company(conn, cfg: Config, s, listed_on: str, day: str, ts: str, listed_shows, markets, aud,
                   rate: float, demand_week: float, ipo_price: int | None = None, float_on_book: bool = True) -> int:
    p, co = cfg.params, cfg.params["company"]
    shares = co["shares"]
    if ipo_price is None:
        d = date.fromisoformat(day)
        share = expected_share(s, d, listed_shows, p, markets, aud)
        fake = {"id": s.id, "shares": shares}
        v = fair_value(conn, cfg, s, d, fake, 0, rate, demand_week, share)
        ipo_price = max(round(co["ipo_price_min"] * 100), round(v * (1 - co["ipo_discount"])))
    n = conn.execute("SELECT COUNT(*) FROM companies").fetchone()[0]
    code = f"{100001 + n}"
    conn.execute("""INSERT INTO companies(id, code, title, title_en, studio, episodes, long_runner, first_air, listed_on,
                    status, shares, ipo_price, prev_close, last_price, fair_value)
                    VALUES (?,?,?,?,?,?,?,?,?, 'listed', ?,?,?,?,?)""",
                 (s.id, code, s.title, s.title_en, s.studio, s.episodes if s.episodes_known else None,
                  int(s.long_runner), s.first_air.isoformat(), listed_on, shares, ipo_price, ipo_price, ipo_price, ipo_price))
    company_acct = conn.execute("INSERT INTO accounts(kind, name, company_id, created_at) VALUES ('company', ?, ?, ?)",
                                (f"{s.title} 公司账户", s.id, ts)).lastrowid
    committee = conn.execute("INSERT INTO accounts(kind, name, company_id, created_at) VALUES ('committee', ?, ?, ?)",
                             (f"{s.title} 制作委员会", s.id, ts)).lastrowid
    world = gdb.special(conn, "world")
    mm = conn.execute("INSERT INTO accounts(kind, name, company_id, created_at) VALUES ('mm', ?, ?, ?)",
                      (f"做市商·{code}", s.id, ts)).lastrowid
    ex.transfer(conn, world, mm, round(cfg.npc["market_maker_cash"] * 100), "grant", day=day, ts=ts, company_id=s.id)
    float_qty = round(shares * co["ipo_float"])
    ex.add_shares(conn, committee, s.id, shares - float_qty, ipo_price, listed_on)
    if float_on_book:
        ex.add_shares(conn, company_acct, s.id, float_qty, ipo_price, listed_on)
        conn.execute("UPDATE holdings SET qty_frozen = ? WHERE account_id=? AND company_id=?", (float_qty, company_acct, s.id))
        conn.execute("""INSERT INTO orders(account_id, company_id, side, kind, price, qty, status, created_at, updated_at)
                        VALUES (?,?, 'sell', 'limit', ?, ?, 'open', ?, ?)""", (company_acct, s.id, ipo_price, float_qty, ts, ts))
        news(conn, day, ts, "ipo", f"新股上市：《{s.title}》（{code}）",
             f"{listed_on} 上市，发行价 {ipo_price / 100:.2f} 币，公开发行 {float_qty:,} 股，先到先得。"
             f"上市后 {cfg.game['ipo_free_days']} 个交易日不设涨跌幅限制。首播日 {s.first_air.isoformat()}。", s.id)
    else:
        ex.add_shares(conn, mm, s.id, float_qty, ipo_price, listed_on)
    return s.id
