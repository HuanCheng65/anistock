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

Everything the close needs from the economic model comes from one file per day, written by
engine/daily_inputs.py (the Python -> Rust interface); the formulas that combine it with game state
are in engine/valuation.py. Nothing here calls the model directly.
"""

from __future__ import annotations

import json
import logging
import math
from datetime import date, timedelta

from engine import daily_inputs as DI
from engine import valuation as V

from . import db as gdb
from . import exchange as ex
from .config import Config

log = logging.getLogger("daily")
MARKET_KINDS = ("player", "mm", "noise", "committee", "company")


# ---------------------------------------------------------------- inputs

def load_inputs(cfg: Config, conn, day: str) -> DI.Inputs:
    """Build the model's inputs for `day` from the latest collector data, keep a copy in
    data/derived/inputs/<day>.json (the day's record, and what a Rust close would read), and parse them
    back from that JSON text, so the close uses exactly what the file says."""
    start = date.fromisoformat(gdb.get_state(conn, "window_start"))
    raw = DI.daily_inputs(day, cfg, window_start=start)
    text = DI.dumps(raw)
    try:
        DI.write(raw)
    except OSError:
        log.exception("could not write the inputs file for %s", day)
    return DI.parse(json.loads(text))


def as_inputs(inputs) -> DI.Inputs:
    """Accept parsed inputs or the raw JSON dict (e.g. json.load of an inputs file)."""
    return inputs if isinstance(inputs, DI.Inputs) else DI.parse(inputs)


def ex_fees(cfg: Config) -> ex.Fees:
    return ex.Fees(commission=cfg.game["commission"], stamp_duty=cfg.game["stamp_duty"])


def account_of(conn, kind: str, company_id: int) -> int:
    return conn.execute("SELECT id FROM accounts WHERE kind=? AND company_id=?", (kind, company_id)).fetchone()["id"]


def news(conn, day: str, ts: str, kind: str, title: str, body: str = "", company_id: int | None = None) -> None:
    conn.execute("INSERT INTO news(ts, day, kind, company_id, title, body) VALUES (?,?,?,?,?,?)",
                 (ts, day, kind, company_id, title, body))


# ---------------------------------------------------------------- valuation

def expected_share(show, listed_shows, inp: DI.Inputs, p) -> float:
    """This show's expected share of external demand, with every listed show's expectation in the denominator."""
    return V.expected_share(show.id, [s.id for s in listed_shows], inp.expected, p["markets"])


def fair_value(conn, cfg: Config, show, d: date, company: dict, book_cents: int, rate: float,
               demand_week: float, pre_share: float) -> int:
    """Per-share value in cents: the simulation's valuation applied to the company's own revenue history."""
    rows = conn.execute("SELECT revenue FROM company_days WHERE company_id=? AND day<=? ORDER BY day DESC LIMIT 7",
                        (company["id"], d.isoformat())).fetchall()
    return V.fair_value_cents([r["revenue"] for r in rows], (d - show.first_air).days + 1, company["shares"],
                              book_cents, rate, demand_week, pre_share, show.aired, show.episodes,
                              show.episodes_known, show.long_runner, show.has_real_data, cfg.params)


# ---------------------------------------------------------------- the step

def run_day(conn, cfg: Config, day: str, ts: str, inputs=None) -> dict:
    """Close `day`. Idempotent per day via state 'last_closed'."""
    if gdb.get_state(conn, "last_closed") and gdb.get_state(conn, "last_closed") >= day:
        return {"skipped": True}
    p, g = cfg.params, cfg.game
    co_p, rv = p["company"], p["revenue"]
    d = date.fromisoformat(day)
    inp = as_inputs(inputs) if inputs is not None else load_inputs(cfg, conn, day)
    if inp.day != day:
        raise ValueError(f"inputs are for {inp.day}, not {day}")
    shows = inp.shows
    world, cb, gov = (gdb.special(conn, k) for k in ("world", "cb", "gov"))
    rate = float(gdb.get_state(conn, "rate"))
    a_ref = float(gdb.get_state(conn, "a_ref"))
    summary = {"day": day, "injected": 0, "exported": 0, "taxes": 0, "interest": 0}

    conn.execute("BEGIN IMMEDIATE")
    try:
        companies = [dict(r) for r in conn.execute("SELECT * FROM companies WHERE status='listed'").fetchall()]
        listed_shows = [shows[c["id"]] for c in companies if c["id"] in shows]

        # 1. revenue
        att = {s.id: s.att for s in listed_shows}
        totals = {m: sum(a[m] for a in att.values()) for m in V.MARKETS}
        tot_us = totals["us"]
        demand_day = V.demand_day(tot_us, a_ref, rv)
        for c in companies:
            s = shows.get(c["id"])
            if s is None:
                continue
            share, parts = V.blended_share(att[s.id], totals, p["markets"])
            rev = round(demand_day * share * 100)
            split = [round(demand_day * parts[m] * 100) for m in V.MARKETS]
            acct = account_of(conn, "company", c["id"])
            ex.transfer(conn, world, acct, rev, "revenue", day=day, ts=ts, company_id=c["id"])
            cost = round(rev * co_p.get("operating_cost_ratio", 0))
            ex.transfer(conn, acct, world, cost, "operating_cost", day=day, ts=ts, company_id=c["id"])
            tax = round((rev - cost) * co_p["corp_tax"])
            ex.transfer(conn, acct, gov, tax, "corp_tax", day=day, ts=ts, company_id=c["id"])
            conn.execute("UPDATE companies SET profit_week = profit_week + ? WHERE id=?", (rev - cost - tax, c["id"]))
            aired = s.aired
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
        demand_week = V.demand_week(tot_us, a_ref, rv)
        total_week_rev = conn.execute("SELECT COALESCE(SUM(revenue),0) FROM company_days WHERE day>?",
                                      ((d - timedelta(days=7)).isoformat(),)).fetchone()[0]
        for c in companies:
            s = shows.get(c["id"])
            if s is None:
                continue
            book = conn.execute("SELECT cash FROM accounts WHERE kind='company' AND company_id=?", (c["id"],)).fetchone()["cash"]
            pre = expected_share(s, listed_shows, inp, p)
            fv = fair_value(conn, cfg, s, d, c, book, rate, demand_week, pre)
            conn.execute("UPDATE companies SET fair_value=? WHERE id=?", (fv, c["id"]))
            if d.weekday() == 6:
                maybe_delist(conn, cfg, day, ts, c, s, total_week_rev, summary)

        # 6. orders are good for the day only (当日有效); the IPO offer stays on the book
        # (only orders from the closing day or earlier: a late close, e.g. after a restart during trading
        # hours, must not cancel what players placed today)
        for o in conn.execute("""SELECT o.* FROM orders o JOIN accounts a ON a.id = o.account_id
                                 WHERE o.status='open' AND a.kind != 'company' AND o.created_at < ?""",
                              (f"{(d + timedelta(days=1)).isoformat()}T{g.get('day_rollover', '00:00')}:00",)).fetchall():
            ex._close(conn, o, "cancelled", ts)

        # 7. candles and next day's reference price
        for c in conn.execute("SELECT * FROM companies WHERE status='listed'").fetchall():
            close_candle(conn, c, day)

        refresh_titles(conn, inp.cn_titles)

        # 8. allot subscriptions that closed today, then open new ones
        for c in conn.execute("SELECT * FROM companies WHERE status='subscribing' AND sub_end<=?", (day,)).fetchall():
            allot(conn, cfg, dict(c), day, ts, summary)
        list_new(conn, cfg, day, ts, inp, listed_shows, rate, demand_week)

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


def revalue_all(conn, cfg: Config, day: str, inputs, rate: float) -> int:
    """Value every listed company with the game's own method (used at launch). Returns how many were valued."""
    p, rv = cfg.params, cfg.params["revenue"]
    d = date.fromisoformat(day)
    inp = as_inputs(inputs)
    if inp.day != day:
        raise ValueError(f"inputs are for {inp.day}, not {day}")
    shows = inp.shows
    a_ref = float(gdb.get_state(conn, "a_ref"))
    companies = [dict(r) for r in conn.execute("SELECT * FROM companies WHERE status='listed'").fetchall()]
    listed_shows = [shows[c["id"]] for c in companies if c["id"] in shows]
    tot_us = sum(s.att["us"] for s in listed_shows)
    demand_week = V.demand_week(tot_us, a_ref, rv)
    n = 0
    for c in companies:
        s = shows.get(c["id"])
        if s is None:
            continue
        book = conn.execute("SELECT cash FROM accounts WHERE kind='company' AND company_id=?", (c["id"],)).fetchone()["cash"]
        pre = expected_share(s, listed_shows, inp, p)
        conn.execute("UPDATE companies SET fair_value=? WHERE id=?",
                     (fair_value(conn, cfg, s, d, c, book, rate, demand_week, pre), c["id"]))
        n += 1
    return n


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
    finished = V.finished(s.aired, s.episodes, s.long_runner)
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
    if conn.execute("SELECT 1 FROM trades WHERE company_id=? AND day>? LIMIT 1", (c["id"], day)).fetchone():
        # a late close while the next day is already trading: keep today's price, anchor and cooling state
        conn.execute("UPDATE companies SET prev_close=? WHERE id=?", (cl, c["id"]))
        return
    conn.execute("""UPDATE companies SET prev_close=?, last_price=?, open_ref=?, vcm_until=NULL, vcm_low=NULL,
                    vcm_high=NULL, vcm_count=0 WHERE id=?""", (cl, cl, cl, c["id"]))


def refresh_titles(conn, cn_titles: dict[int, str]) -> None:
    """Pick up Chinese titles that appeared after listing (new Bangumi entries, manual fixes)."""
    for cid, name in cn_titles.items():
        conn.execute("UPDATE companies SET title=? WHERE id=? AND title != ?", (name, cid, name))


def list_new(conn, cfg: Config, day: str, ts: str, inp: DI.Inputs, listed_shows: list,
             rate: float, demand_week: float) -> None:
    """Open IPO subscriptions for shows that list within the subscription window. A show lists
    `ipo_lead_days` before its premiere; its subscription runs the `ipo_subscription_days` before that."""
    p, g = cfg.params, cfg.game
    tomorrow = date.fromisoformat(day) + timedelta(days=1)
    lead = p["universe"]["ipo_lead_days"]
    window = g["ipo_subscription_days"]
    existing = {r["id"] for r in conn.execute("SELECT id FROM companies").fetchall()}
    for s in inp.shows.values():
        if s.id in existing or s.popularity_end < g["min_listing_popularity"]:
            continue
        if not (s.first_air - timedelta(days=lead + window) <= tomorrow):
            continue
        if s.first_air < tomorrow - timedelta(days=60) and not s.long_runner:
            continue                                   # long finished before the game noticed it
        listed_on = max(s.first_air - timedelta(days=lead), tomorrow + timedelta(days=window))
        create_company(conn, cfg, s, listed_on.isoformat(), day, ts, inp, listed_shows, rate, demand_week,
                       subscription=(tomorrow.isoformat(), (listed_on - timedelta(days=1)).isoformat()))


def create_company(conn, cfg: Config, s, listed_on: str, day: str, ts: str, inp: DI.Inputs, listed_shows,
                   rate: float, demand_week: float, ipo_price: int | None = None,
                   subscription: tuple[str, str] | None = None) -> int:
    """Create a company with its accounts. With `subscription` (start, end) it opens an IPO subscription
    and the public float is created at allotment; without it (launch seeding) the market maker holds the float."""
    p, co = cfg.params, cfg.params["company"]
    shares = co["shares"]
    title = inp.cn_titles.get(s.id) or s.title_ja
    if ipo_price is None:
        d = date.fromisoformat(day)
        share = expected_share(s, listed_shows, inp, p)
        fake = {"id": s.id, "shares": shares}
        v = fair_value(conn, cfg, s, d, fake, 0, rate, demand_week, share)
        ipo_price = max(round(co["ipo_price_min"] * 100), round(v * (1 - co["ipo_discount"])))
    n = conn.execute("SELECT COUNT(*) FROM companies").fetchone()[0]
    code = f"{100001 + n}"
    status = "subscribing" if subscription else "listed"
    sub_start, sub_end = subscription or (None, None)
    conn.execute("""INSERT INTO companies(id, code, title, title_ja, title_en, studio, episodes, long_runner, first_air,
                    listed_on, status, sub_start, sub_end, shares, ipo_price, prev_close, last_price, open_ref, fair_value)
                    VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)""",
                 (s.id, code, title, s.title_ja, s.title_en, s.studio, s.episodes if s.episodes_known else None,
                  int(s.long_runner), s.first_air.isoformat(), listed_on, status, sub_start, sub_end, shares,
                  ipo_price, ipo_price, ipo_price, ipo_price, ipo_price))
    conn.execute("INSERT INTO accounts(kind, name, company_id, created_at) VALUES ('company', ?, ?, ?)",
                 (f"{title} 公司账户", s.id, ts))
    committee = conn.execute("INSERT INTO accounts(kind, name, company_id, created_at) VALUES ('committee', ?, ?, ?)",
                             (f"{title} 制作委员会", s.id, ts)).lastrowid
    world = gdb.special(conn, "world")
    mm = conn.execute("INSERT INTO accounts(kind, name, company_id, created_at) VALUES ('mm', ?, ?, ?)",
                      (f"做市商·{code}", s.id, ts)).lastrowid
    ex.transfer(conn, world, mm, round(cfg.npc["market_maker_cash"] * 100), "grant", day=day, ts=ts, company_id=s.id)
    float_qty = round(shares * co["ipo_float"])
    ex.add_shares(conn, committee, s.id, shares - float_qty, ipo_price, listed_on)
    if subscription:
        base = round(float_qty * cfg.game["ipo_public_base"])
        news(conn, day, ts, "ipo", f"新股认购：《{title}》（{code}）",
             f"{sub_start} 至 {sub_end} 开放认购，发行价 {ipo_price / 100:.2f} 币，公开发售 {base:,} 股"
             f"（超额认购 15/50/100 倍时回拨到 70%/80%/90% 的流通盘）。{listed_on} 上市，首播日 {s.first_air.isoformat()}。",
             s.id)
    else:
        ex.add_shares(conn, mm, s.id, float_qty, ipo_price, listed_on)
    return s.id


def public_share(cfg: Config, oversubscribed: float) -> float:
    """Share of the float that goes to the public after clawback (回拨)."""
    frac = cfg.game["ipo_public_base"]
    for multiple, f in cfg.game["ipo_clawback"]:
        if oversubscribed >= multiple:
            frac = f
    return frac


def allot(conn, cfg: Config, c: dict, day: str, ts: str, summary: dict) -> None:
    """Close a subscription: allot pro rata with a ballot for the odd shares, collect payment, release the
    rest, give unsold shares to the rights holders, pay the studio fee, and list the company."""
    import random
    fees = ex.Fees(commission=cfg.game["commission"], stamp_duty=cfg.game["stamp_duty"])
    world = gdb.special(conn, "world")
    company_acct = account_of(conn, "company", c["id"])
    committee = account_of(conn, "committee", c["id"])
    price = c["ipo_price"]
    float_qty = round(c["shares"] * cfg.params["company"]["ipo_float"])
    base = round(float_qty * cfg.game["ipo_public_base"])
    apps = [dict(r) for r in conn.execute("SELECT * FROM ipo_applications WHERE company_id=? ORDER BY created_at, account_id",
                                          (c["id"],)).fetchall()]
    total = sum(a["qty"] for a in apps)
    over = total / base if base else 0.0
    public = min(total, round(float_qty * public_share(cfg, over)))
    if total <= public:
        for a in apps:
            a["allotted"] = a["qty"]
    else:
        ratio = public / total
        for a in apps:
            a["allotted"] = math.floor(a["qty"] * ratio)
        left = public - sum(a["allotted"] for a in apps)
        rng = random.Random(c["id"])                  # the ballot (抽签) for the odd shares
        while left > 0:
            pool = [a for a in apps if a["allotted"] < a["qty"]]
            if not pool:
                break
            a = rng.choices(pool, weights=[x["qty"] - x["allotted"] for x in pool])[0]
            a["allotted"] += 1
            left -= 1
    proceeds = 0
    for a in apps:
        conn.execute("UPDATE accounts SET cash_frozen = cash_frozen - ? WHERE id=?", (a["frozen"], a["account_id"]))
        cost = a["allotted"] * price
        if cost:
            ex.transfer(conn, a["account_id"], company_acct, cost, "ipo", day=day, ts=ts, company_id=c["id"])
            ex.transfer(conn, a["account_id"], world, fees.commission_on(cost), "commission", day=day, ts=ts, company_id=c["id"])
            ex.add_shares(conn, a["account_id"], c["id"], a["allotted"], price, c["listed_on"])
            proceeds += cost
        conn.execute("UPDATE ipo_applications SET allotted=?, frozen=0 WHERE company_id=? AND account_id=?",
                     (a["allotted"], c["id"], a["account_id"]))
    allotted = sum(a["allotted"] for a in apps)
    ex.add_shares(conn, committee, c["id"], float_qty - allotted, price, c["listed_on"])   # unsold: founders keep them
    fee = round(proceeds * cfg.params["company"]["production_fee_ratio"])
    ex.transfer(conn, company_acct, world, fee, "production_fee", day=day, ts=ts, company_id=c["id"])
    summary["exported"] += fee
    conn.execute("UPDATE companies SET status='listed' WHERE id=?", (c["id"],))
    rate_txt = f"中签率 {allotted / total:.1%}" if total else "无人认购"
    news(conn, day, ts, "allotment", f"《{c['title']}》认购结果：超额认购 {over:.1f} 倍",
         f"共 {len(apps)} 人认购 {total:,} 股，公开发售 {public:,} 股（流通盘的 {public_share(cfg, over):.0%}），{rate_txt}。"
         f"未售出的 {float_qty - allotted:,} 股由制作委员会持有。{c['listed_on']} 上市交易。", c["id"])
