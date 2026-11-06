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
import sys
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
sys.path.insert(0, str(ROOT))
from engine.economy import (  # noqa: E402
    MARKETS, Show, at, attention, blended_share, expected_att, find_gap_days, fundamental,
    has_real_data, load_audiences, load_shows, market_att, preair_att, score_mult,
)
def simulate(shows: list[Show], start: date, end: date, p: dict, markets: dict, aud: dict) -> dict:
    rng = random.Random(p["seed"])
    rv, co, gv, cb, mk, val = (p["revenue"], p["company"], p["government"],
                               p["central_bank"], p["market"], p["valuation"])
    weights = p["markets"]
    shares = co["shares"]

    hh_cash = p["economy"]["players"] * p["economy"]["initial_cash"] + p["economy"].get("npc_cash", 0)
    exported = 0.0    # money that left the game economy today (rights holders' dividends, studio fees)
    founders = co.get("founder_share", 0.0)
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

    # (a replay shorter than a week, the start world of server/tools/rebuild_replay.py: the days it has)
    n_ref = min(7, (end - start).days + 1)
    a_ref = sum(total_attention(start + timedelta(days=k)) for k in range(n_ref)) / n_ref or 1.0

    out_days, macro, stocks = [], [], {s.id: [] for s in shows}
    tax_week = 0.0
    m2_hist = []
    total_prev = hh_cash + bills + sum(co_cash.values()) + gov

    d = start
    while d <= end:
        created = 0.0
        injected = 0.0
        exported = 0.0
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
                exp_div = exp_rev * (1 - co.get("operating_cost_ratio", 0.0)) * (1 - co["corp_tax"]) * co["payout_ratio"] / shares
                v0 = fundamental(s, d, exp_div, 0.0, rate, p, exp_div, use_preair=not has_real_data(s, d, val))
                ipo_price = max(co["ipo_price_min"], v0 * (1 - co["ipo_discount"]))
                raise_amt = ipo_price * shares * co["ipo_float"]   # founders keep the rest, no cash changes hands
                hh_cash -= raise_amt
                fee = raise_amt * co["production_fee_ratio"]
                co_cash[s.id] += raise_amt - fee
                if co.get("fees_leave_economy"):
                    exported += fee          # paid to the real-world studio
                else:
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
            # operating costs (distribution, studio, marketing) are paid to outside parties
            cost = rev * co.get("operating_cost_ratio", 0.0)
            exported += cost
            profit = rev - cost
            tax = profit * co["corp_tax"]
            co_cash[sid] += profit - tax
            gov += tax
            taxes_today["corp"] += tax
            profit_week[sid] += profit - tax
            revenue_hist[sid].append(rev)

        # 3. weekly dividends (Sunday)
        if d.weekday() == 6:
            for sid in listed:
                div = max(0.0, profit_week[sid] * co["payout_ratio"])
                div = min(div, co_cash[sid])
                co_cash[sid] -= div
                # rights holders (制作委员会) own `founders` of the shares; their dividends leave the economy
                out = div * founders
                exported += out
                t = (div - out) * co["dividend_tax"]
                gov += t
                taxes_today["dividend"] += t
                hh_cash += div - out - t
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
            preair_div = preair_weekly * (1 - co.get("operating_cost_ratio", 0.0)) * (1 - co["corp_tax"]) * co["payout_ratio"] / shares
            if since_premiere < 7:
                # premiere week: real revenue for the days that happened, the pre-air expectation for the
                # rest. Scaling 2 days of premiere spike by 7/2 would overshoot.
                hist = revenue_hist[s.id][-max(1, since_premiere):]
                weekly_rev = sum(hist) + preair_weekly / 7 * (7 - len(hist))
            else:
                hist = revenue_hist[s.id][-7:]
                weekly_rev = sum(hist) * 7 / max(1, len(hist))
            weekly_div = weekly_rev * (1 - co.get("operating_cost_ratio", 0.0)) * (1 - co["corp_tax"]) * co["payout_ratio"] / shares
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
                "book": round(book, 4),
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
            L = cb["growth_lookback_days"]
            if len(m2_hist) > L:
                g = (m2_hist[-1] / m2_hist[-1 - L]) ** (365 / L) - 1
                want = cb["r_neutral"] + cb["k_money"] * (g - cb["m2_growth_target"])
                want = min(cb["r_max"], max(cb["r_min"], want))
                rate += max(-cb["max_step"], min(cb["max_step"], want - rate))

        # conservation check
        total = hh_cash + bills + sum(co_cash.values()) + gov
        expected = total_prev + injected + created - exported
        assert abs(total - expected) < 1e-6 * max(1.0, total), f"ledger leak on {d}: {total - expected}"
        assert hh_cash >= 0 and gov >= 0, f"negative balance on {d}: households={hh_cash:.0f} gov={gov:.0f}"
        total_prev = total

        L = cb["growth_lookback_days"]
        g4 = (m2_hist[-1] / m2_hist[-1 - L]) ** (365 / L) - 1 if len(m2_hist) > L else None
        macro.append({
            "d": d.isoformat(), "rate": round(rate, 5), "m2": round(m2, 2), "m2_growth_ann": g4 and round(g4, 4),
            "hh_cash": round(hh_cash, 2), "bills": round(bills, 2), "co_cash": round(sum(co_cash.values()), 2),
            "gov": round(gov, 2), "injected": round(injected, 2), "interest": round(created, 2),
            "tax_corp": round(taxes_today["corp"], 2), "tax_div": round(taxes_today["dividend"], 2),
            "tax_stamp": round(taxes_today["stamp"], 2), "transfer": round(transfer, 2),
            "mcap": round(mcap, 2), "listed": len(listed), "exported": round(exported, 2),
        })
        out_days.append(d)
        d += timedelta(days=1)

    meta = {s.id: {"title": s.title, "title_en": s.title_en, "studio": s.studio, "format": s.fmt,
                   "episodes": s.episodes, "first_air": s.first_air.isoformat(), "list_day": s.list_day.isoformat(),
                   "popularity_end": s.popularity_end} for s in shows}
    return {"meta": meta, "stocks": stocks, "macro": macro}


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
