# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""AniStock economy simulation, v1 (AniList data only).

Reads the collector DB read-only, builds a daily economy and writes sim/out/result.json.

    uv run sim/simulate.py [--params sim/params.toml] [--db data/anistock.db]

Sectors and money:
    households  players' cash (deposits) + central bank bills
    companies   one per anime IP; revenue comes from outside (real viewers' attention)
    government  collects taxes, pays part back as transfers
    central bank sets the policy rate, issues bills, creates interest

    total money = households cash + bills + company cash + government balance
    Δ total     = external revenue + interest created      (checked every day)
    M2          = households cash + company cash            (bills and gov balance excluded)
"""

from __future__ import annotations

import argparse
import json
import math
import random
import sqlite3
import tomllib
from bisect import bisect_right
from collections import defaultdict
from dataclasses import dataclass, field
from datetime import date, datetime, timedelta, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


# ---------------------------------------------------------------- data

@dataclass
class Show:
    id: int
    title: str
    title_en: str | None
    fmt: str
    episodes: int | None             # None = long-runner / unknown
    popularity_end: int
    studio: str | None
    days: list[date] = field(default_factory=list)
    popularity: list[float] = field(default_factory=list)
    in_progress: list[float] = field(default_factory=list)
    trending: list[float] = field(default_factory=list)
    score: list[float | None] = field(default_factory=list)
    aired: list[int] = field(default_factory=list)          # episodes aired as of each day
    first_air: date | None = None
    list_day: date | None = None
    long_runner: bool = False
    episodes_known: bool = True


def load_shows(db_path: Path, p: dict, start: date, end: date | None) -> tuple[list[Show], date]:
    db = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    latest = {}
    for mid, data in db.execute(
            "SELECT external_id, data FROM snapshots WHERE source='anilist' AND kind='media' ORDER BY day"):
        latest[int(mid)] = json.loads(data)   # later days overwrite earlier ones

    trends: dict[int, dict[date, dict]] = {}
    for mid, d, data in db.execute("SELECT media_id, date, data FROM anilist_trends"):
        trends.setdefault(mid, {})[date.fromisoformat(d)] = json.loads(data)

    max_day = max(d for t in trends.values() for d in t)
    if end is None:
        end = max_day - timedelta(days=1)    # the last day is usually partial
    gaps = find_gap_days(trends)

    u = p["universe"]
    shows = []
    for mid, m in latest.items():
        if m.get("format") not in u["formats"] or (m.get("popularity") or 0) < u["min_popularity"]:
            continue
        t = trends.get(mid)
        if not t:
            continue
        s = Show(
            id=mid,
            title=m["title"].get("native") or m["title"].get("romaji") or str(mid),
            title_en=m["title"].get("english") or m["title"].get("romaji"),
            fmt=m["format"],
            episodes=m.get("episodes"),
            popularity_end=m.get("popularity") or 0,
            studio=(m.get("studios", {}).get("nodes") or [{}])[0].get("name"),
        )
        _fill_grid(s, t, end, gaps)
        if not s.days:
            continue
        ep_days = sorted(d for d, n in t.items() if n.get("episode") is not None)
        sd = m.get("startDate") or {}
        start_date = date(sd["year"], sd["month"], sd["day"]) if sd.get("year") and sd.get("month") and sd.get("day") else None
        s.first_air = min([d for d in ep_days[:1]] + ([start_date] if start_date else [])) if (ep_days or start_date) else None
        if s.first_air is None:
            continue
        s.list_day = max(start, s.first_air - timedelta(days=u["ipo_lead_days"]))
        v = p["valuation"]
        s.long_runner = (end - s.first_air).days > 365 * v["long_runner_years"]
        s.episodes_known = s.episodes is not None
        if s.episodes is None and not s.long_runner:
            s.episodes = v["default_episodes"]
        if s.list_day > end:
            continue
        shows.append(s)
    return shows, end


def find_gap_days(trends: dict[int, dict[date, dict]]) -> set[date]:
    """Days AniList lost data for everyone (e.g. 2026-07-06/07 have no rows at all). A day is a gap when
    it has under half the usual row count, or its total trending is under 25% of the surrounding week's."""
    import statistics
    rows, trend = defaultdict(int), defaultdict(float)
    for t in trends.values():
        for d, n in t.items():
            rows[d] += 1
            trend[d] += n.get("trending") or 0
    if not rows:
        return set()
    days = sorted(rows)
    gaps = set()
    d = days[0]
    while d <= days[-1]:
        near = [rows.get(d + timedelta(days=k), 0) for k in range(-7, 8) if k]
        near_t = [trend[d + timedelta(days=k)] for k in range(-3, 4) if k and rows.get(d + timedelta(days=k), 0)]
        if rows.get(d, 0) < 0.5 * statistics.median(near) or (near_t and trend[d] < 0.25 * statistics.median(near_t)):
            gaps.add(d)
        d += timedelta(days=1)
    return gaps


def _fill_grid(s: Show, t: dict[date, dict], end: date, gaps: set[date] = frozenset()) -> None:
    """Dense daily grid. Stocks (popularity, inProgress, score) forward-fill; trending is a flow, so a show's
    quiet day is 0 — except on market-wide gap days, where the previous day's value is carried forward."""
    first = min(t)
    pop = inprog = 0.0
    score = None
    aired = 0
    d = first
    while d <= end:
        n = t.get(d)
        if n:
            pop = n.get("popularity") or pop
            inprog = n.get("inProgress") if n.get("inProgress") is not None else inprog
            sc = n.get("averageScore")
            score = sc if sc else score               # 0 means "not enough votes yet"
            if n.get("episode") is not None:
                aired = max(aired, n["episode"])
        s.days.append(d)
        s.popularity.append(float(pop))
        s.in_progress.append(float(inprog))
        if d in gaps and s.trending:
            s.trending.append(s.trending[-1])
        else:
            s.trending.append(float(n.get("trending") or 0) if n else 0.0)
        s.score.append(score)
        s.aired.append(aired)
        d += timedelta(days=1)


def at(s: Show, series: list, d: date, default=0.0):
    i = (d - s.days[0]).days
    if i < 0:
        return default
    return series[min(i, len(series) - 1)]


# ---------------------------------------------------------------- economics

def score_mult(score: float | None, r: dict) -> float:
    if not score:
        return 1.0
    return min(r["score_mult_max"], max(r["score_mult_min"], (score / r["score_ref"]) ** r["score_power"]))


def attention(s: Show, d: date, r: dict) -> float:
    i = (d - s.days[0]).days
    if i < 0:
        return 0.0
    new_adds = max(0.0, s.popularity[i] - s.popularity[i - 1]) if i > 0 else 0.0
    a = r["w_in_progress"] * s.in_progress[i] / 7 + r["w_new_adds"] * new_adds + r["w_trending"] * s.trending[i]
    return a * score_mult(s.score[i], r)


MARKETS = ("us", "cn", "jp")


def market_att(s: Show, d: date, r: dict, markets: dict) -> dict[str, float]:
    """Attention of one show on one day in each market. us = AniList (observed daily);
    cn/jp = rebuilt weekly series from sim/reconstruct.py. Score premium (AniList) applies to all."""
    a = {"us": attention(s, d, r)}
    row = markets.get(s.id, {}).get(d.isoformat(), {})
    mult = score_mult(at(s, s.score, d, None), r)
    a["cn"] = row.get("cn", 0.0) * mult
    a["jp"] = row.get("jp", 0.0) * mult
    return a


def blended_share(own: dict[str, float], totals: dict[str, float], weights: dict) -> tuple[float, dict]:
    """Σ_m w_m * own_m/T_m, renormalised over markets that have any data. Returns (share, per-market part)."""
    live = [m for m in MARKETS if totals[m] > 0]
    wsum = sum(weights[m] for m in live) or 1.0
    parts = {m: (weights[m] / wsum) * own[m] / totals[m] if m in live else 0.0 for m in MARKETS}
    return sum(parts.values()), parts


def preair_att(s: Show, d: date, val: dict, aud: dict) -> dict[str, float]:
    """Expected daily attention per market once the show airs, from audience sizes before airing."""
    a = aud.get(s.id, {})
    return {"us": val["preair_attention_per_user"] * at(s, s.popularity, d) / 7,
            "cn": val["preair_cn_per_member"] * a.get("cn", 0) / 7,
            "jp": val["preair_jp_per_member"] * a.get("jp", 0) / 7}


def has_real_data(s: Show, d: date, val: dict) -> bool:
    """Premiered long enough ago that every market has reported (cn/jp lag the premiere by a day).
    Keyed on the premiere date, not AniList episode markers: long-runners such as Doraemon have none."""
    return (d - s.first_air).days >= val["min_days_after_premiere"]


def expected_att(s: Show, d: date, r: dict, markets: dict, val: dict, aud: dict) -> dict[str, float]:
    """What a market participant expects a show to draw: observed once it airs, else the larger of
    observed and the audience-based pre-air estimate. Pre-air shows must sit in the denominator too,
    or every unaired show looks like it owns its whole market."""
    a = market_att(s, d, r, markets)
    if has_real_data(s, d, val):
        return a
    pre = preair_att(s, d, val, aud)
    return {m: max(a[m], pre[m]) for m in MARKETS}


def fundamental(s: Show, d: date, weekly_div: float, book: float, rate: float, p: dict,
                preair_div: float, use_preair: bool = False) -> float:
    """Per-share value: discounted expected dividends + book value per share.

    Expected weekly dividend runs at the current level, falls once by `premiere_drop` if only the
    premiere has aired, then by `in_season_retention` per week until the finale, then follows the
    measured post-finale profile."""
    v = p["valuation"]
    rw = (rate + v["equity_premium"]) / 52
    disc = 1 / (1 + rw)
    aired = at(s, s.aired, d, 0)
    div = preair_div if use_preair else weekly_div
    if s.long_runner:
        g = v["long_runner_decay"]
        pv = div * g * disc / (1 - g * disc)
    else:
        remaining = max(0, s.episodes - aired)
        if not s.episodes_known and aired > 0:
            remaining = max(remaining, v["min_remaining_weeks"])
        rho = v["in_season_retention"]
        level = div * (v["premiere_drop"] if aired == 1 and not use_preair else 1.0)
        pv = 0.0
        for k in range(1, remaining + 1):
            pv += level * rho ** (k - 1) * disc ** k
        last = level * rho ** max(0, remaining - 1)
        prof, tail = v["post_finale_profile"], v["post_finale_tail_decay"]
        for j, f in enumerate(prof, start=1):
            pv += last * f * disc ** (remaining + j)
        n = remaining + len(prof)
        pv += last * prof[-1] * (disc ** n) * (tail * disc) / (1 - tail * disc)
    return pv + book


# ---------------------------------------------------------------- simulation

def simulate(shows: list[Show], start: date, end: date, p: dict, markets: dict, aud: dict) -> dict:
    rng = random.Random(p["seed"])
    rv, co, gv, cb, mk, val = (p["revenue"], p["company"], p["government"],
                               p["central_bank"], p["market"], p["valuation"])
    weights = p["markets"]
    shares = co["shares"]

    hh_cash = p["economy"]["players"] * p["economy"]["initial_cash"]
    bills = 0.0
    gov = 0.0
    rate = cb["r_neutral"]
    co_cash = {s.id: 0.0 for s in shows}
    profit_week = {s.id: 0.0 for s in shows}
    revenue_hist = {s.id: [] for s in shows}     # daily revenue, for trailing 7-day run-rate
    rev_by_market = {s.id: {m: 0.0 for m in MARKETS} for s in shows}  # today's revenue split
    price = {}
    prev_price = {}
    listed: set[int] = set()

    # reference attention for the demand index: first 7 days of the window
    def total_attention(d):
        return sum(attention(s, d, rv) for s in shows if s.list_day <= d)

    a_ref = sum(total_attention(start + timedelta(days=k)) for k in range(7)) / 7 or 1.0

    out_days, macro, stocks = [], [], {s.id: [] for s in shows}
    tax_week = 0.0
    m2_hist = []
    total_prev = hh_cash + bills + sum(co_cash.values()) + gov

    d = start
    while d <= end:
        created = 0.0
        injected = 0.0
        taxes_today = {"corp": 0.0, "dividend": 0.0, "stamp": 0.0}

        # 1. IPOs, priced at fundamental value minus the IPO discount
        # denominator = trailing-week attention of everything that will be listed after today's IPOs,
        # measured the same way as the newcomer's own attention (otherwise same-day listers misprice)
        listing_today = [x for x in shows if x.list_day == d and x.id not in listed]
        if listing_today:
            def trailing(x):
                acc = {m: 0.0 for m in MARKETS}
                for k in range(1, 8):
                    for m, v in expected_att(x, d - timedelta(days=k), rv, markets, val, aud).items():
                        acc[m] += v / 7
                return acc
            trail = {x.id: trailing(x) for x in shows if x.id in listed or x in listing_today}
            tot_prev = {m: sum(t[m] for t in trail.values()) for m in MARKETS}
            if tot_prev["us"] <= 0:
                tot_prev["us"] = a_ref
        for s in shows:
            if s.list_day == d and s.id not in listed:
                own_share, _ = blended_share(trail[s.id], tot_prev, weights)
                exp_rev = rv["demand_base_week"] * own_share
                exp_div = exp_rev * (1 - co["corp_tax"]) * co["payout_ratio"] / shares
                v0 = fundamental(s, d, exp_div, 0.0, rate, p, exp_div, use_preair=not has_real_data(s, d, val))
                ipo_price = max(co["ipo_price_min"], v0 * (1 - co["ipo_discount"]))
                raise_amt = ipo_price * shares * co["ipo_float"]   # founders keep the rest, no cash changes hands
                hh_cash -= raise_amt
                fee = raise_amt * co["production_fee_ratio"]
                co_cash[s.id] += raise_amt - fee
                hh_cash += fee
                listed.add(s.id)
                price[s.id] = prev_price[s.id] = ipo_price

        # 2. external revenue, split by attention share
        att = {s.id: market_att(s, d, rv, markets) for s in shows if s.id in listed}
        totals = {m: sum(a[m] for a in att.values()) for m in MARKETS}
        tot = totals["us"]   # the demand index uses AniList only: the markets' units are not comparable
        demand_day = rv["demand_base_week"] / 7 * ((tot / a_ref) ** rv["demand_elasticity"] if tot > 0 else 0)
        for sid, a in att.items():
            share, parts = blended_share(a, totals, weights)
            rev = demand_day * share
            rev_by_market[sid] = {m: demand_day * parts[m] for m in MARKETS}
            injected += rev
            tax = rev * co["corp_tax"]
            co_cash[sid] += rev - tax
            gov += tax
            taxes_today["corp"] += tax
            profit_week[sid] += rev - tax
            revenue_hist[sid].append(rev)

        # 3. weekly dividends (Sunday)
        if d.weekday() == 6:
            for sid in listed:
                div = max(0.0, profit_week[sid] * co["payout_ratio"])
                div = min(div, co_cash[sid])
                co_cash[sid] -= div
                t = div * co["dividend_tax"]
                gov += t
                taxes_today["dividend"] += t
                hh_cash += div - t
                profit_week[sid] = 0.0

        # 4. interest (created by the central bank)
        r_dep = max(0.0, rate - cb["deposit_spread"])
        i_dep = hh_cash * r_dep / 365
        i_bill = bills * rate / 365
        hh_cash += i_dep + i_bill
        created += i_dep + i_bill

        # 5. households shift between deposits and bills as the rate moves
        liquid = hh_cash + bills
        target = min(cb["bill_share_max"], max(0.0, cb["bill_share_base"] + cb["bill_share_slope"] * (rate - cb["r_neutral"])))
        move = (target * liquid - bills) * cb["bill_adjust_speed"]
        move = max(-bills, min(hh_cash, move))
        bills += move
        hh_cash -= move

        # 6. valuation + market
        exp_tot = {m: 0.0 for m in MARKETS}
        exp_cache = {}
        for x in shows:
            if x.id in listed:
                exp_cache[x.id] = expected_att(x, d, rv, markets, val, aud)
                for m in MARKETS:
                    exp_tot[m] += exp_cache[x.id][m]
        mcap = 0.0
        for s in shows:
            if s.id not in listed:
                continue
            since_premiere = (d - s.first_air).days + 1
            use_pre = not has_real_data(s, d, val)
            pre_share, _ = blended_share(exp_cache[s.id], exp_tot, weights)
            preair_weekly = demand_day * 7 * pre_share
            preair_div = preair_weekly * (1 - co["corp_tax"]) * co["payout_ratio"] / shares
            if since_premiere < 7:
                # premiere week: real revenue for the days that happened, the pre-air expectation for the
                # rest. Scaling 2 days of premiere spike by 7/2 would overshoot.
                hist = revenue_hist[s.id][-max(1, since_premiere):]
                weekly_rev = sum(hist) + preair_weekly / 7 * (7 - len(hist))
            else:
                hist = revenue_hist[s.id][-7:]
                weekly_rev = sum(hist) * 7 / max(1, len(hist))
            weekly_div = weekly_rev * (1 - co["corp_tax"]) * co["payout_ratio"] / shares
            book = co_cash[s.id] / shares
            V = fundamental(s, d, weekly_div, book, rate, p, preair_div, use_preair=use_pre)

            p0, pp = price[s.id], prev_price[s.id]
            free = ((d - s.list_day).days < mk["ipo_free_days"]
                    or 0 <= (d - s.first_air).days < mk["airing_free_days"])
            lim = math.inf if free else mk["limit"]
            step = mk["kappa"] * (V - p0) + mk["mu"] * (p0 - pp) + mk["sigma"] * p0 * rng.gauss(0, 1)
            new = max(0.01, min(p0 * (1 + lim), max(p0 * (1 - lim), p0 + step)))
            op = min(p0 * (1 + lim), max(p0 * (1 - lim), p0 * (1 + rng.gauss(0, mk["gap_sigma"]))))
            hi = max(op, new) * (1 + abs(rng.gauss(0, mk["sigma"] / 3)))
            lo = min(op, new) * (1 - abs(rng.gauss(0, mk["sigma"] / 3)))
            hi, lo = min(hi, p0 * (1 + lim)), max(lo, p0 * (1 - lim))
            prev_price[s.id], price[s.id] = p0, new
            mcap += new * shares
            stocks[s.id].append({
                "d": d.isoformat(), "o": round(op, 4), "h": round(hi, 4), "l": round(lo, 4), "pc": round(p0, 4),
                "c": round(new, 4), "v": round(V, 4), "rev": round(revenue_hist[s.id][-1], 2),
                "div_w": round(weekly_div, 4), "aired": at(s, s.aired, d, 0),
                "rm": [round(rev_by_market[s.id][m], 2) for m in MARKETS],
            })

        # 7. stamp duty on daily turnover (sellers are households)
        stamp = mcap * mk["daily_turnover"] * mk["stamp_duty"]
        stamp = min(stamp, hh_cash)
        hh_cash -= stamp
        gov += stamp
        taxes_today["stamp"] = stamp
        tax_week += sum(taxes_today.values())

        # 8. weekly: transfers and the rate decision (Sunday, after dividends)
        transfer = 0.0
        m2 = hh_cash + sum(co_cash.values())
        m2_hist.append(m2)
        if d.weekday() == 6:
            transfer = min(gov, tax_week * gv["transfer_ratio"])
            gov -= transfer
            hh_cash += transfer
            tax_week = 0.0
            m2 = hh_cash + sum(co_cash.values())
            m2_hist[-1] = m2
            if len(m2_hist) > 28:
                g = (m2_hist[-1] / m2_hist[-29]) ** (365 / 28) - 1
                want = cb["r_neutral"] + cb["k_money"] * (g - cb["m2_growth_target"])
                want = min(cb["r_max"], max(cb["r_min"], want))
                rate += max(-cb["max_step"], min(cb["max_step"], want - rate))

        # conservation check
        total = hh_cash + bills + sum(co_cash.values()) + gov
        expected = total_prev + injected + created
        assert abs(total - expected) < 1e-6 * max(1.0, total), f"ledger leak on {d}: {total - expected}"
        assert hh_cash >= 0 and gov >= 0, f"negative balance on {d}: households={hh_cash:.0f} gov={gov:.0f}"
        total_prev = total

        g4 = (m2_hist[-1] / m2_hist[-29]) ** (365 / 28) - 1 if len(m2_hist) > 28 else None
        macro.append({
            "d": d.isoformat(), "rate": round(rate, 5), "m2": round(m2, 2), "m2_growth_ann": g4 and round(g4, 4),
            "hh_cash": round(hh_cash, 2), "bills": round(bills, 2), "co_cash": round(sum(co_cash.values()), 2),
            "gov": round(gov, 2), "injected": round(injected, 2), "interest": round(created, 2),
            "tax_corp": round(taxes_today["corp"], 2), "tax_div": round(taxes_today["dividend"], 2),
            "tax_stamp": round(taxes_today["stamp"], 2), "transfer": round(transfer, 2),
            "mcap": round(mcap, 2), "listed": len(listed),
        })
        out_days.append(d)
        d += timedelta(days=1)

    meta = {s.id: {"title": s.title, "title_en": s.title_en, "studio": s.studio, "format": s.fmt,
                   "episodes": s.episodes, "first_air": s.first_air.isoformat(), "list_day": s.list_day.isoformat(),
                   "popularity_end": s.popularity_end} for s in shows}
    return {"meta": meta, "stocks": stocks, "macro": macro}


def load_audiences(db_path: Path) -> dict[int, dict[str, float]]:
    """Bangumi (想看+在看+看过) and Annict watchers per AniList id, from the latest snapshot."""
    mfile = ROOT / "sim" / "matches.json"
    if not mfile.exists():
        return {}
    matches = json.loads(mfile.read_text(encoding="utf-8"))["matches"]
    db = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    bg = {int(i): json.loads(x) for i, x in db.execute(
        "SELECT external_id, data FROM snapshots WHERE source='bgm' AND kind='subject' ORDER BY day")}
    an = {int(i): json.loads(x) for i, x in db.execute(
        "SELECT external_id, data FROM snapshots WHERE source='annict' AND kind='work' ORDER BY day")}
    out = {}
    for aid, r in matches.items():
        a = {}
        if r.get("bgm") in bg:
            c = bg[r["bgm"]].get("collection") or {}
            a["cn"] = c.get("wish", 0) + c.get("doing", 0) + c.get("collect", 0)
        if r.get("annict") in an:
            a["jp"] = an[r["annict"]].get("watchersCount", 0)
        out[int(aid)] = a
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--params", default=str(ROOT / "sim" / "params.toml"))
    ap.add_argument("--db", default=str(ROOT / "data" / "anistock.db"))
    ap.add_argument("--out", default=str(ROOT / "sim" / "out" / "result.json"))
    ap.add_argument("--set", action="append", default=[], metavar="section.key=value",
                    help="override a parameter, e.g. --set central_bank.max_step=0.01")
    args = ap.parse_args()

    p = tomllib.loads(Path(args.params).read_text(encoding="utf-8"))
    for item in args.set:
        path, _, raw = item.partition("=")
        *sections, key = path.split(".")
        node = p
        for sec in sections:
            node = node[sec]
        node[key] = tomllib.loads(f"v = {raw}")["v"]
    start = date.fromisoformat(p["window"]["start"])
    end = date.fromisoformat(p["window"]["end"]) if p["window"]["end"] else None
    shows, end = load_shows(Path(args.db), p, start, end)
    markets = {}
    mpath = ROOT / "data" / "derived" / "markets.json"
    if mpath.exists():
        markets = {int(k): v for k, v in json.loads(mpath.read_text(encoding="utf-8"))["series"].items()}
    else:
        print("data/derived/markets.json missing: running AniList-only (run sim/reconstruct.py build)")
    aud = load_audiences(Path(args.db))
    res = simulate(shows, start, end, p, markets, aud)
    res["markets_loaded"] = bool(markets)
    res["params"] = p
    res["window"] = {"start": start.isoformat(), "end": end.isoformat()}
    res["generated_at"] = datetime.now(timezone.utc).isoformat(timespec="seconds")

    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(res, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")

    mac = res["macro"]
    print(f"window {start}..{end}, listed {mac[-1]['listed']} shows")
    for row in mac[::7] + [mac[-1]]:
        g = row["m2_growth_ann"]
        print(f"  {row['d']} rate={row['rate']:.4f} M2={row['m2']:>12,.0f} "
              f"growth={'   n/a' if g is None else f'{g:6.1%}'} bills={row['bills']:>11,.0f} "
              f"gov={row['gov']:>10,.0f} mcap={row['mcap']:>12,.0f}")
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
