"""One-time value-equivalence check of API v1 against the legacy API (docs/api-v1.md §7.3, step 6).

    uv run python backend/difftest/v1_equiv.py [--bin backend/target/release/anistock] [--work DIR]

The Rust server still serves both APIs. For each scenario (the API difftest's seeded database and frozen
clock: open, auction, closed) two servers start on two copies of the same database: "old" answers the
legacy requests, "new" the /api/v1 ones. Every read is requested on both and the legacy response is
converted by the table in §7.7 (amounts × 100, renamed fields, StockRef unfolded, split / merged
endpoints recombined), then compared value by value. Then the same writes go through the old API on one
server and through v1 on the other (orders, cancels, a market order, IPO subscription changes, a new
account), the reads are compared again, and finally every table of both databases.

Allowed differences are listed in ALLOWED and reported, not failed. This script is removed with the
legacy API.
"""

from __future__ import annotations

import argparse
import json
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import api_diff as ad  # noqa: E402

ALLOWED = {
    "ratios rounding": "v1 rounds ratios to 6 decimals, the v2 read-only API to 4 (§2.5)",
    "holders": "v1 holder buckets are fixed (player, committee, unissued, market_maker, npc, other) (§4.2.1)",
    "restructure message": "the 破产重整 refusal text follows docs/copywriting.md",
    "ipo status": "v1 IpoStatus distinguishes upcoming / open / allotted / listed (§4.0)",
    "airings window": "v1 cuts airings by trading day (04:00), the old /api/v2/tonight by 06:00 (§4.1.5)",
    "audiences": "v1 drops anilist.next_episode / cover and bangumi.name_cn, types collection (§4.2.1), and reads a "
                 "snapshot with \"genres\": null as no genres where the legacy endpoint dropped the whole anilist block",
}

# the API difftest's "closed" (23:30) is inside the night session now; 05:00 is the closed market
ad.SCENARIOS["closed0500"] = dict(time="2026-11-06T05:00:00+08:00", always_open=False, dist=False, secret=ad.SECRET)
SCEN = ["open", "auction", "closed", "closed0500"]

# the check sends far more requests per second than the per-IP limit allows
_init = ad.Server.__init__


def _no_limits(self, name, cmd, env, log):
    env["ANISTOCK_NO_RATE_LIMIT"] = "1"
    _init(self, name, cmd, env, log)


ad.Server.__init__ = _no_limits
OK, DIFF, ALLOW = [], [], []


def c100(v):
    return None if v is None else int(round(v * 100))


def close(a, b, tol=1e-6):
    if a is None or b is None:
        return a is None and b is None
    return abs(a - b) <= tol * max(1.0, abs(a), abs(b))


class Ctx:
    def __init__(self, old: int, new: int):
        self.old, self.new = old, new
        self.ck_old, self.ck_new = None, None

    def o(self, method, path, body=None):
        r = ad.call(self.old, method, path, body=body, cookie=self.ck_old)
        return r.status, (json.loads(r.body) if r.body and r.headers.get("content-type", "").startswith("application/json") else None), r

    def n(self, method, path, body=None):
        r = ad.call(self.new, method, path, body=body, cookie=self.ck_new)
        return r.status, (json.loads(r.body) if r.body and r.headers.get("content-type", "").startswith("application/json") else None), r


def eq(label, a, b, allowed: str | None = None, tol=None):
    same = close(a, b, tol) if (tol is not None and isinstance(a, (int, float)) and isinstance(b, (int, float))) else a == b
    if same:
        OK.append(label)
    elif allowed:
        ALLOW.append((label, allowed, a, b))
    else:
        DIFF.append((label, a, b))


def ratio(label, a, b, rnd_allowed=True):
    """A ratio: equal to 1e-9, or within the 4-decimal rounding of the old value (allowed)."""
    if a is None or b is None:
        eq(label, a, b)
    elif abs(a - b) <= 1e-9 * max(1, abs(a)):
        OK.append(label)
    elif rnd_allowed and abs(a - b) <= 0.5e-4 * max(1, abs(b)) + 1e-12:
        ALLOW.append((label, "ratios rounding", a, b))
    else:
        DIFF.append((label, a, b))


# ---------------------------------------------------------------- reads

def reads(x: Ctx, tag: str):
    # status / rules
    _, s, _ = x.o("GET", "/api/status")
    _, ms, _ = x.n("GET", "/api/v1/market/status")
    _, ru, _ = x.n("GET", "/api/v1/rules")
    for k, v in [("trading_day", s["day"]), ("phase", s["phase"]), ("last_closed_day", s["last_closed"]),
                 ("launched_on", s["launched_on"]), ("policy_rate", s["rate"]), ("now", s["now"])]:
        eq(f"{tag} status.{k}", v, ms[k])
    eq(f"{tag} status.deposit_rate", s["deposit_rate"], ms["deposit_rate"], tol=1e-9)
    r = s["rules"]
    eq(f"{tag} rules.commission", r["commission"], ru["fees"]["commission"])
    eq(f"{tag} rules.stamp_duty", r["stamp_duty"], ru["fees"]["stamp_duty"])
    eq(f"{tag} rules.vcm", (r["vcm_threshold"], r["vcm_window_min"], r["vcm_cool_min"]),
       (ru["vcm"]["threshold"], ru["vcm"]["window_minutes"], ru["vcm"]["cool_minutes"]))
    eq(f"{tag} rules.initial_cash", c100(r["initial_cash"]), ru["initial_cash"])
    eq(f"{tag} rules.dividend_tax", [b[1] for b in r["dividend_tax"]], [b["rate"] for b in ru["dividend_tax"]])
    eq(f"{tag} rules.session", (s["auction_start"], s["open"], s["close"], s["rollover"]),
       (ru["session"]["auction_start"], ru["session"]["open"], ru["session"]["close"], ru["session"]["day_rollover"]))

    # market + ratios -> quotes
    _, mk, _ = x.o("GET", "/api/market")
    _, rt, _ = x.o("GET", "/api/v2/ratios")
    _, q, _ = x.n("GET", "/api/v1/market/quotes")
    rmap = {r["id"]: r for r in rt["rows"]}
    eq(f"{tag} quotes.order", [m["id"] for m in mk["stocks"]], [i["stock"]["id"] for i in q["items"]])
    for m, i in zip(mk["stocks"], q["items"]):
        L = f"{tag} quotes[{m['id']}]"
        eq(L + ".ref", (m["id"], m["code"], m["title"]), (i["stock"]["id"], i["stock"]["code"], i["stock"]["title"]))
        for a, b in [("title_ja", "title_ja"), ("title_en", "title_en"), ("volume", "volume"), ("episodes", "episodes"),
                     ("long_runner", "is_long_runner"), ("first_air", "first_air"), ("listed_on", "listed_on"),
                     ("ipo_left", "ipo_remaining"), ("new", "is_new"), ("cooling", "vcm_active"), ("aired", "episodes_aired")]:
            eq(f"{L}.{b}", m.get(a), i[b])
        for a, b in [("price", "price"), ("prev_close", "prev_close"), ("turnover", "turnover"), ("market_cap", "market_cap"),
                     ("week_revenue", "week_revenue"), ("ipo_price", "ipo_price")]:
            eq(f"{L}.{b}", c100(m[a]), i[b])
        eq(f"{L}.sparkline", [c100(v) for v in m["spark"]], i["sparkline"])
        ratio(f"{L}.change", m["change"], i["change"], rnd_allowed=True)
        ratio(f"{L}.week_revenue_change", m["revenue_change"], i["week_revenue_change"])
        rr = rmap.get(m["id"], {})
        for a, b in [("pe", "pe_ttm"), ("pe_dyn", "pe_dynamic"), ("dividend_yield", "dividend_yield_ttm"),
                     ("dividend_yield_dyn", "dividend_yield_dynamic")]:
            ratio(f"{L}.ratios.{b}", rr.get(a), i["ratios"][b])

    # each listed stock (and the subscribing / delisted ones the seed has)
    ids = [m["id"] for m in mk["stocks"]] + [105, 106]
    for sid in ids:
        stock(x, tag, sid)

    # indices / tonight / macro / listings
    _, ix, _ = x.o("GET", "/api/v2/indices")
    _, ni, _ = x.n("GET", "/api/v1/market/indices")
    eq(f"{tag} indices.breadth", ix["breadth"], ni["breadth"])
    eq(f"{tag} indices.season_start", ix["season_start"], ni["season_start"])
    for a, b in zip(ix["indices"], ni["items"]):
        L = f"{tag} indices.{a['key']}"
        eq(L + ".meta", (a["key"], a["name"], a["members"], a["base_day"], a["value"], a["prev"]),
           (b["key"], b["name"], b["members"], b["base_day"], b["value"], b["prev_close"]))
        ratio(L + ".change", a["change"], b["change"])
        eq(L + ".series", [[d, v] for d, v in a["series"]], [[p["trading_day"], p["value"]] for p in b["series"]])
    _, tn, _ = x.o("GET", "/api/v2/tonight")
    _, na, _ = x.n("GET", "/api/v1/market/airings")
    old_air = {(a["company_id"], a["episode"], a["airing_at"]) for a in tn["airings"]}
    new_air = {(a["stock"]["id"], a["episode"], a["airs_at"]) for a in na["items"]}
    eq(f"{tag} airings", sorted(old_air), sorted(new_air), allowed="airings window")
    _, mo, _ = x.o("GET", "/api/macro")
    _, mv, _ = x.o("GET", "/api/v2/macro")
    _, nm, _ = x.n("GET", "/api/v1/market/macro?from=2000-01-01")
    nmap = {r["trading_day"]: r for r in nm["items"]}
    for d in mo["days"]:
        r = nmap.get(d["day"], {})
        eq(f"{tag} macro[{d['day']}]", (d["rate"], c100(d["m2"]), c100(d["injected"]), c100(d["exported"]), c100(d["taxes"])),
           (r.get("policy_rate"), r.get("m2"), r.get("injected"), r.get("exported"), r.get("taxes")))
        ratio(f"{tag} macro.m2_growth[{d['day']}]", d["m2_growth"], r.get("m2_growth"))
    for d, v in mv["revenue_week"]:
        eq(f"{tag} macro.revenue_7d[{d}]", c100(v), nmap.get(d, {}).get("revenue_7d"))
    for d, v in mv["demand"]:
        ratio(f"{tag} macro.demand[{d}]", v, nmap.get(d, {}).get("demand_index"))
    _, li, _ = x.o("GET", "/api/v2/listings")
    _, nl, _ = x.n("GET", "/api/v1/ipos?status=upcoming,listed")
    nmap = {i["stock"]["id"]: i for i in nl["items"]}
    for l in li["recent"]:
        i = nmap.get(l["id"])
        eq(f"{tag} listings[{l['id']}].present", True, i is not None)
        if i:
            eq(f"{tag} listings[{l['id']}]", (c100(l["ipo_price"]), c100(l["first_close"]), l["listed_on"]),
               (i["price"], i["first_day"] and i["first_day"]["close"], i["listed_on"]))
    for u in li["upcoming"]:
        eq(f"{tag} upcoming[{u['id']}]", u["sub_start"], nmap.get(u["id"], {}).get("sub_start"))

    # news, leaderboard, ipos
    _, nw, _ = x.o("GET", "/api/news?limit=200")
    _, nn, _ = x.n("GET", "/api/v1/news?limit=200")
    eq(f"{tag} news", [(n["id"], n["day"], n["ts"], n["kind"], n["title"], n["body"] or None, n["company_id"], n["code"]) for n in nw["news"]],
       [(n["id"], n["trading_day"], n["at"], n["kind"], n["title"], n["body"], n["stock"] and n["stock"]["id"], n["stock"] and n["stock"]["code"]) for n in nn["items"]])
    _, lb, _ = x.o("GET", "/api/leaderboard")
    _, nb, _ = x.n("GET", "/api/v1/leaderboard")
    eq(f"{tag} leaderboard", [(p["rank"], p["username"], c100(p["total"]), p["restructures"]) for p in lb["players"]],
       [(p["rank"], p["user"]["username"], p["total_assets"], p["restructures"]) for p in nb["items"]])
    for p, n in zip(lb["players"], nb["items"]):
        ratio(f"{tag} leaderboard.return[{p['username']}]", p["return_pct"], n["return"])
    ipos(x, tag)
    if x.ck_old:
        account(x, tag)


def ipos(x, tag):
    _, io, _ = x.o("GET", "/api/ipos")
    for o in io["ipos"]:
        _, n, _ = x.n("GET", f"/api/v1/ipos/{o['id']}")
        L = f"{tag} ipo[{o['id']}]"
        eq(L, (o["code"], o["title"], o["title_ja"], c100(o["ipo_price"]), o["sub_start"], o["sub_end"], o["listed_on"], o["first_air"],
               o["float_qty"], o["base_public_qty"], o["applied_qty"], o["applicants"]),
           (n["stock"]["code"], n["stock"]["title"], n["title_ja"], n["price"], n["sub_start"], n["sub_end"], n["listed_on"], n["first_air"],
            n["float_quantity"], n["base_public_quantity"], n["applied_quantity"], n["applicants"]))
        eq(L + ".status", o["status"], n["status"], allowed="ipo status")
        if o["result"]:
            r = n["result"] or {}
            eq(L + ".result", (o["result"]["public_qty"],), (r.get("public_quantity"),))
            ratio(L + ".oversubscribed", o["result"]["oversubscribed"], r.get("oversubscribed"))
            ratio(L + ".allotment_ratio", o["result"]["ratio"], r.get("allotment_ratio"))
        else:
            eq(L + ".result", None, n["result"])
        if x.ck_old:
            my = o["my"]
            v = n["viewer"]
            eq(L + ".viewer", (my["qty"], my["allotted"]) if my else (0, None), (v["quantity"], v["allotted"]) if v else None)


def stock(x: Ctx, tag: str, sid: int):
    s, o, _ = x.o("GET", f"/api/stocks/{sid}")
    s2, n, _ = x.n("GET", f"/api/v1/stocks/{sid}")
    L = f"{tag} stock[{sid}]"
    eq(L + ".status", s, s2)
    if s != 200:
        return
    _, p, _ = x.o("GET", f"/api/v2/stocks/{sid}/profile")
    eq(L + ".ref", (o["id"], o["code"], o["title"]), (n["stock"]["id"], n["stock"]["code"], n["stock"]["title"]))
    for a, b in [("title_ja", "title_ja"), ("title_en", "title_en"), ("studio", "studio"), ("status", "status"), ("episodes", "episodes"),
                 ("long_runner", "is_long_runner"), ("first_air", "first_air"), ("listed_on", "listed_on"), ("shares", "shares")]:
        eq(f"{L}.{b}", o[a], n[b])
    eq(L + ".ipo_price", c100(o["ipo_price"]), n["ipo_price"])
    eq(L + ".ipo_remaining", o["ipo_left"], n["quote"]["ipo_remaining"])
    oq, nq = o["quote"], n["quote"]
    eq(L + ".quote", tuple(c100(oq[k]) for k in ("price", "prev_close", "open", "high", "low", "turnover", "open_ref")) + (oq["volume"],),
       tuple(nq[k] for k in ("price", "prev_close", "open", "high", "low", "turnover", "open_reference")) + (nq["volume"],))
    ov, nv = o["vcm"], n["vcm"]
    eq(L + ".vcm", (ov["active"], ov["until"], c100(ov["low"]), c100(ov["high"]), ov["count"]),
       (nv["active"], nv["until"] and nv["until"], nv["low"], nv["high"], nv["count_today"]))
    eq(L + ".weeks", [(w["week"], c100(w["revenue"]), c100(w["us"]), c100(w["cn"]), c100(w["jp"]), c100(w["dividend_ps"]), w["aired"]) for w in o["weeks"]],
       [(w["week_start"], w["revenue"], w["by_market"]["us"], w["by_market"]["cn"], w["by_market"]["jp"], w["dividend_per_share"], w["episodes_aired"]) for w in n["revenue_weeks"]])
    eq(L + ".book_value_ps", c100(o["book_value_ps"]), n["ratios"]["book_value_per_share"])
    ratio(L + ".pb", p["ratios"]["pb"], n["ratios"]["pb"])
    ratio(L + ".pe", p["ratios"]["pe"], n["ratios"]["pe_ttm"])
    eq(L + ".episode_days", [(e["n"], e["day"]) for e in p["episodes"]], [(e["episode"], e["trading_day"]) for e in n["episode_days"]])
    eq(L + ".dividends", [(d["day"], c100(d["ps"])) for d in p["dividends"]], [(d["trading_day"], d["per_share"]) for d in n["dividends"]])
    eq(L + ".next_episode", p["next_episode"] and (p["next_episode"]["episode"], p["next_episode"]["airing_at"]),
       n["next_episode"] and (n["next_episode"]["episode"], n["next_episode"]["airs_at"]))
    h = n["holders"]
    legacy = dict(o["holders"])
    mapped = {"player": legacy.pop("player", 0), "committee": legacy.pop("committee", 0), "unissued": legacy.pop("company", 0),
              "market_maker": legacy.pop("mm", 0)}
    npc = legacy.pop("npc", 0) + legacy.pop("noise", 0)
    eq(L + ".holders", (mapped, npc, sum(legacy.values())), ({k: h[k] for k in mapped}, h["npc"]["total"], h["other"]), allowed="holders")
    if any(o["holders"].values()):
        eq(L + ".holders.sum", sum(v for k, v in h.items() if k != "npc") + h["npc"]["total"], n["shares"])
    a = o["audiences"]
    na = n["audiences"]
    eq(L + ".audiences.anilist", a.get("anilist") and (a["anilist"]["popularity"], a["anilist"]["score"], a["anilist"]["genres"]),
       na["anilist"] and (na["anilist"]["popularity"], na["anilist"]["score"], na["anilist"]["genres"]), allowed="audiences")
    eq(L + ".audiences.bangumi", a.get("bangumi") and (a["bangumi"]["score"], a["bangumi"]["collection"]),
       na["bangumi"] and (na["bangumi"]["score"], na["bangumi"]["collection"]), allowed="audiences")
    eq(L + ".audiences.annict", a.get("annict") and a["annict"]["watchers"], na["annict"] and na["annict"]["watchers"])
    # sub-resources
    _, bk, _ = x.n("GET", f"/api/v1/stocks/{sid}/book")
    eq(L + ".book", {k: [(c100(l["price"]), l["qty"], l["orders"]) for l in o["book"][k]] for k in ("bids", "asks")},
       {k: [(l["price"], l["quantity"], l["orders"]) for l in bk[k]] for k in ("bids", "asks")})
    if o["auction"] is None:
        eq(L + ".auction", None, bk["auction"])
    else:
        eq(L + ".auction", (c100(o["auction"]["price"]), o["auction"]["volume"]), (bk["auction"]["price"], bk["auction"]["quantity"]))
    _, tr, _ = x.n("GET", f"/api/v1/stocks/{sid}/trades?limit=30")
    eq(L + ".trades", [(c100(t["price"]), t["qty"], t["ts"]) for t in o["trades"]], [(t["price"], t["quantity"], t["at"]) for t in tr["items"]])
    _, nw, _ = x.n("GET", f"/api/v1/news?stock_id={sid}&limit=20")
    eq(L + ".news", [(z["id"], z["day"], z["kind"], z["title"], z["body"] or None) for z in o["news"]],
       [(z["id"], z["trading_day"], z["kind"], z["title"], z["body"]) for z in nw["items"]])
    if o.get("ipo"):
        eq(L + ".ipo", (o["ipo"]["applied_qty"], o["ipo"]["applicants"]), (n["ipo"]["applied_quantity"], n["ipo"]["applicants"]))
    else:
        eq(L + ".ipo", None, n["ipo"])
    _, cd, _ = x.o("GET", f"/api/stocks/{sid}/candles")
    _, nc, _ = x.n("GET", f"/api/v1/stocks/{sid}/candles")
    eq(L + ".candles", [(k["time"], c100(k["open"]), c100(k["high"]), c100(k["low"]), c100(k["close"]), k["volume"], "replay" if k["replay"] else "live", bool(k.get("live"))) for k in cd["candles"]],
       [(k["trading_day"], k["open"], k["high"], k["low"], k["close"], k["volume"], k["source"], k["is_partial"]) for k in nc["items"]])
    _, it, _ = x.o("GET", f"/api/stocks/{sid}/intraday")
    _, ni, _ = x.n("GET", f"/api/v1/stocks/{sid}/intraday")
    eq(L + ".intraday", [(z["minute"], c100(z["price"]), z["volume"]) for z in it["points"]], [(z["minute"][:16], z["price"], z["volume"]) for z in ni["items"]])
    eq(L + ".intraday.prev_close", c100(oq["prev_close"]), ni["prev_close"])
    if x.ck_old:
        _, pf, _ = x.n("GET", "/api/v1/me/portfolio")
        pos = next((z for z in pf["items"] if z["stock"]["id"] == sid), None)
        eq(L + ".position", o["position"] and (o["position"]["qty"], c100(o["position"]["value"])), pos and (pos["quantity"], pos["value"]))
        _, mo, _ = x.n("GET", f"/api/v1/me/orders?stock_id={sid}")
        eq(L + ".my_orders", [z["id"] for z in o["my_orders"]], [z["id"] for z in mo["items"]])
        _, me, _ = x.n("GET", "/api/v1/me")
        eq(L + ".available_cash", c100(o["available_cash"]), me["available_cash"])


def account(x: Ctx, tag: str):
    _, me, _ = x.o("GET", "/api/me")
    _, nm, _ = x.n("GET", "/api/v1/me")
    eq(f"{tag} me", (me["id"], me["username"], c100(me["cash"]), c100(me["frozen"]), c100(me["available"]), c100(me["stock_value"]),
                     c100(me["total"]), c100(me["initial"]), c100(me["dividends"]), me["restructures"], me["can_restructure"]),
       (nm["id"], nm["username"], nm["cash"], nm["frozen_cash"], nm["available_cash"], nm["stock_value"], nm["total_assets"],
        nm["initial_cash"], nm["dividends"], nm["restructure"]["count"], nm["restructure"]["eligible"]))
    eq(f"{tag} me.total_return", me["return_pct"], nm["total_return"])
    eq(f"{tag} me.deposit_rate", me["deposit_rate"], nm["deposit_rate"], tol=1e-9)
    eq(f"{tag} me.restructure_reason", me["restructure_reason"], nm["restructure"]["message"], allowed="restructure message")
    _, pf, _ = x.o("GET", "/api/portfolio")
    _, np_, _ = x.n("GET", "/api/v1/me/portfolio")
    eq(f"{tag} portfolio.summary", tuple(c100(pf[k]) for k in ("cash", "frozen", "available", "stock_value", "total")),
       tuple(np_["summary"][k] for k in ("cash", "frozen_cash", "available_cash", "stock_value", "total_assets")))
    eq(f"{tag} portfolio.items", [(z["company_id"], z["code"], z["title"], z["qty"], z["available"], c100(z["avg_cost"]), c100(z["price"]), c100(z["value"]),
                                   c100(z["pnl"]), c100(z["dividends"]), z["oldest_lot"]) for z in pf["positions"]],
       [(z["stock"]["id"], z["stock"]["code"], z["stock"]["title"], z["quantity"], z["available_quantity"], z["average_cost"], z["price"], z["value"],
         z["pnl"], z["dividends"], z["oldest_lot_on"]) for z in np_["items"]])
    for z, w in zip(pf["positions"], np_["items"]):
        ratio(f"{tag} portfolio.pnl_ratio[{z['company_id']}]", z["pnl_pct"], w["pnl_ratio"])
        ratio(f"{tag} portfolio.day_change[{z['company_id']}]", z["day_change"], w["day_change"])
    for st in ("open", "all"):
        _, od, _ = x.o("GET", f"/api/orders?status={st}")
        _, no, _ = x.n("GET", f"/api/v1/me/orders?status={st}&limit=200")
        eq(f"{tag} orders[{st}]", [(z["id"], z["company_id"], z["code"], z["title"], z["side"], z["kind"], c100(z["price"]), z["qty"], z["filled"], z["status"], z["created_at"], z["updated_at"]) for z in od["orders"]],
           [(z["id"], z["stock"]["id"], z["stock"]["code"], z["stock"]["title"], z["side"], z["type"], z["price"], z["quantity"], z["filled_quantity"], z["status"], z["created_at"], z["updated_at"]) for z in no["items"]])
    _, tr, _ = x.o("GET", "/api/trades")
    _, nt, _ = x.n("GET", "/api/v1/me/trades?limit=200")
    eq(f"{tag} trades", [(z["id"], z["company_id"], z["code"], z["title"], z["side"], c100(z["price"]), z["qty"], z["ts"]) for z in tr["trades"]],
       [(z["id"], z["stock"]["id"], z["stock"]["code"], z["stock"]["title"], z["side"], z["price"], z["quantity"], z["at"]) for z in nt["items"]])
    _, lg, _ = x.o("GET", "/api/ledger")
    _, nl, _ = x.n("GET", "/api/v1/me/ledger?limit=200")
    eq(f"{tag} ledger", [(z["ts"], z["reason"], z["title"], c100(z["amount"])) for z in lg["entries"]],
       [(z["at"], z["reason"], z["stock"] and z["stock"]["title"], z["amount"]) for z in nl["items"]])


# ---------------------------------------------------------------- writes

def writes(x: Ctx, tag: str, phase: str):
    """The same operations through both APIs; results compared, then the databases."""
    pairs = []
    if phase != "closed":
        pairs += [
            ("buy limit", ("POST", "/api/orders", {"company_id": 102, "side": "buy", "kind": "limit", "price": "12.30", "qty": 7}),
             ("POST", "/api/v1/me/orders", {"stock_id": 102, "side": "buy", "type": "limit", "price": 1230, "quantity": 7})),
            ("sell limit", ("POST", "/api/orders", {"company_id": 101, "side": "sell", "kind": "limit", "price": "5.20", "qty": 3}),
             ("POST", "/api/v1/me/orders", {"stock_id": 101, "side": "sell", "type": "limit", "price": 520, "quantity": 3})),
            ("buy crossing", ("POST", "/api/orders", {"company_id": 101, "side": "buy", "kind": "limit", "price": "5.10", "qty": 4}),
             ("POST", "/api/v1/me/orders", {"stock_id": 101, "side": "buy", "type": "limit", "price": 510, "quantity": 4})),
        ]
        if phase == "continuous":
            pairs.append(("market buy", ("POST", "/api/orders", {"company_id": 101, "side": "buy", "kind": "market", "qty": 2}),
                          ("POST", "/api/v1/me/orders", {"stock_id": 101, "side": "buy", "type": "market", "quantity": 2})))
    pairs += [
        ("subscribe", ("POST", "/api/ipos/106/subscribe", {"qty": 40}), ("PUT", "/api/v1/ipos/106/subscription", {"quantity": 40})),
        ("subscribe again", ("POST", "/api/ipos/106/subscribe", {"qty": 25}), ("PUT", "/api/v1/ipos/106/subscription", {"quantity": 25})),
    ]
    for label, (om, op, ob), (nm, np_, nb) in pairs:
        so, jo, _ = x.o(om, op, ob)
        sn, jn, _ = x.n(nm, np_, nb)
        eq(f"{tag} write {label} ok", so == 200, sn in (200, 201))
        if so != 200:
            print(f"   {label}: old {so} {jo}  new {sn} {jn}")
        if "orders" in op and so == 200:
            eq(f"{tag} write {label}", (jo["status"], jo["filled"], c100(jo["avg_price"])),
               (jn["order"]["status"], jn["order"]["filled_quantity"], jn["order"]["average_fill_price"]))
    # cancel the first resting order on each side
    _, od, _ = x.o("GET", "/api/orders?status=open")
    _, nd, _ = x.n("GET", "/api/v1/me/orders?status=open")
    if od["orders"]:
        so, _, _ = x.o("DELETE", f"/api/orders/{od['orders'][-1]['id']}")
        sn, jn, _ = x.n("DELETE", f"/api/v1/me/orders/{nd['items'][-1]['id']}")
        eq(f"{tag} write cancel", so == 200, sn == 200)
    # errors that both refuse
    for label, (om, op, ob), (nm, np_, nb), code in [
        ("insufficient cash", ("POST", "/api/orders", {"company_id": 102, "side": "buy", "kind": "limit", "price": "12.30", "qty": 9000000}),
         ("POST", "/api/v1/me/orders", {"stock_id": 102, "side": "buy", "type": "limit", "price": 1230, "quantity": 9000000}),
         "insufficient_cash" if phase != "closed" else "market_closed"),
        ("unknown stock", ("POST", "/api/orders", {"company_id": 1, "side": "buy", "kind": "limit", "price": "1", "qty": 1}),
         ("POST", "/api/v1/me/orders", {"stock_id": 1, "side": "buy", "type": "limit", "price": 100, "quantity": 1}),
         "stock_not_found" if phase != "closed" else "market_closed"),
    ]:
        so, jo, _ = x.o(om, op, ob)
        sn, jn, _ = x.n(nm, np_, nb)
        eq(f"{tag} error {label}", so >= 400, sn >= 400)
        eq(f"{tag} error {label} code", code, jn and jn.get("code"))
    # unsubscribe and a new account
    so, _, _ = x.o("DELETE", "/api/ipos/106/subscribe")
    sn, _, _ = x.n("DELETE", "/api/v1/ipos/106/subscription")
    eq(f"{tag} write unsubscribe", so == 200, sn == 200)
    so, _, _ = x.o("POST", "/api/register", {"username": "carol", "password": "123456", "invite": ad.INVITE})
    sn, _, _ = x.n("POST", "/api/v1/auth/register", {"username": "carol", "password": "123456", "invite": ad.INVITE})
    eq(f"{tag} write register", so == 200, sn == 201)


def login(x: Ctx):
    r = ad.call(x.old, "POST", "/api/login", body={"username": "p0", "password": "secret0"})
    x.ck_old = r.cookie()
    r = ad.call(x.new, "POST", "/api/v1/auth/login", body={"username": "p0", "password": "secret0"})
    x.ck_new = r.cookie()


def run(name: str, binary: str, work: Path) -> None:
    sc = ad.SCENARIOS[name]
    w = work / name
    w.mkdir(parents=True, exist_ok=True)
    root = ad.make_root(w, False)
    base, coll = ad.seed(w, root, False)
    dbs = {k: w / f"{k}.db" for k in ("v1", "v2")}
    for p in dbs.values():
        p.write_bytes(base.read_bytes())
    servers = {k: ad.start("v2", binary, root, dbs[k], coll, sc, w / f"{k}.log") if k == "v2" else None for k in ("v2",)}
    old_srv = servers["v2"]
    ad.PORTS["v2b"] = ad.PORTS["v2"] + 10
    env_port = ad.PORTS["v2"]
    ad.PORTS["v2"] = ad.PORTS["v2b"]
    new_srv = ad.start("v2", binary, root, dbs["v1"], coll, sc, w / "new.log")
    ad.PORTS["v2"] = env_port
    try:
        x = Ctx(old_srv.port, new_srv.port)
        reads(x, f"{name}/anon")
        login(x)
        reads(x, f"{name}/p0")
        writes(x, f"{name}/p0", x.n("GET", "/api/v1/market/status")[1]["phase"])
        reads(x, f"{name}/after")
    finally:
        old_srv.stop()
        new_srv.stop()
    print(f"[{name}] database tables:")
    bad = ad.compare_dbs(dbs["v2"], dbs["v1"], base, gensecret=False)
    eq(f"{name} databases", 0, bad)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", default=str(ad.REPO / "backend" / "target" / "release" / "anistock"))
    ap.add_argument("--work", default=None)
    ap.add_argument("--scenario", nargs="*", default=SCEN)
    a = ap.parse_args()
    work = Path(a.work or tempfile.mkdtemp(prefix="v1equiv-"))
    for s in a.scenario:
        run(s, a.bin, work)
    print(f"\n{len(OK)} values equal, {len(ALLOW)} allowed differences, {len(DIFF)} differences")
    seen = set()
    for label, why, a1, b1 in ALLOW:
        key = (why, label.split("/")[-1].split("[")[0].split(".")[0])
        if key in seen:
            continue
        seen.add(key)
        print(f"  allowed ({why}): {label}: {str(a1)[:120]} | {str(b1)[:120]}")
    for label, a1, b1 in DIFF:
        print(f"  DIFF {label}:\n     old {str(a1)[:300]}\n     new {str(b1)[:300]}")
    print("\nallowed difference classes:")
    for k in sorted({w for _, w, _, _ in ALLOW}):
        print(f"  - {k}: {ALLOWED[k]}")
    sys.exit(1 if DIFF else 0)


if __name__ == "__main__":
    main()
