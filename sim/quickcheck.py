"""Fast check of the valuation (layer 1): mechanical strategies on the hidden-value paths, with confidence intervals.

    python3 sim/quickcheck.py [--world DB] [--inputs DIR] [--collector DB] [--out DIR] [--no-run]
                              [--save NAME] [--compare NAME] [--reps N] [--set key=value ...]

Runs `marketsim --value-only` (the daily closes alone, no intraday trading: about 2 seconds for a season) and reads
each stock's hidden value and dividends per day. The hidden value and the dividends depend only on the inputs, not
on trading (backend/src/sim/mod.rs), and prices track the value closely (docs/v2-series.md: deviation p95 about
10%, half-life about 16 days; a bot's picks earn the value's own total return), so whether a mechanical strategy
beats the market can be measured on the value paths without simulating the market. The full replay (`marketsim`
with bots) stays the check of the NPC market; this one is for iterating on the valuation.

Each strategy decides at a day's close from what is known then (that day's inputs and values), buys at that value
and sells at a later value, collecting the dividends in between. Its excess return is against the cap-weighted
market over the same window. Strategies are formed on many days (not one start day), and the interval is a
cluster bootstrap over stocks: a result carried by two or three stocks shows up as a wide interval.

Also reports the size of the daily value revisions (what the opening auction prices in), their autocorrelation
and the dividend-yield level.

--save NAME keeps the summary in data/fast/NAME.json; --compare NAME prints the difference to a saved one.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import random
import statistics as st
import subprocess
import sys
import time
from datetime import date, timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FAST = ROOT / "data" / "fast"
WEIGHTS = {"cn": 0.5, "us": 0.3, "jp": 0.2}  # server/game.toml [revenue] market weights (NPC heat uses the same)


# ---------------------------------------------------------------- data

def run_value_only(args) -> Path:
    exe = ROOT / "backend" / "target" / "release" / "marketsim"
    if not exe.exists():
        sys.exit(f"{exe} missing: cargo build --release --bin marketsim (in backend/)")
    cmd = [str(exe), "--world", str(args.world), "--inputs", str(args.inputs), "--value-only", "--out", str(args.out)]
    if args.collector and Path(args.collector).exists():
        cmd += ["--collector", str(args.collector)]
    for kv in args.set:
        cmd += ["--set", kv]
    t = time.time()
    r = subprocess.run(cmd, cwd=ROOT, env=dict(os.environ, ANISTOCK_ROOT=str(ROOT)), capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit(f"marketsim failed:\n{r.stderr[-2000:]}")
    print(f"value-only replay: {time.time() - t:.1f}s")
    return Path(args.out)


def heat_by_day(inputs: Path, days: list[str]) -> dict[str, dict[int, tuple[float, float]]]:
    """Per day, per show: (the NPCs' heat = weighted share of `expected`, the public heat = weighted share of the
    observed `att`). Cached in data/fast/heat.json by the inputs' modification times."""
    files = [inputs / f"{d}.json" for d in days]
    key = [f.name + ":" + str(int(f.stat().st_mtime)) for f in files if f.exists()]
    cache = FAST / "heat.json"
    if cache.exists():
        c = json.loads(cache.read_text())
        if c.get("key") == key:
            return {d: {int(k): tuple(v) for k, v in m.items()} for d, m in c["heat"].items()}
    out = {}
    for f in files:
        if not f.exists():
            continue
        shows = json.loads(f.read_text())["shows"]
        tot_e = {m: sum(s["expected"].get(m, 0) for s in shows) for m in WEIGHTS}
        tot_a = {m: sum(s["att"].get(m, 0) for s in shows) for m in WEIGHTS}
        out[f.stem] = {s["id"]: (sum(w * s["expected"].get(m, 0) / tot_e[m] for m, w in WEIGHTS.items() if tot_e[m] > 0),
                                 sum(w * s["att"].get(m, 0) / tot_a[m] for m, w in WEIGHTS.items() if tot_a[m] > 0))
                       for s in shows}
    FAST.mkdir(parents=True, exist_ok=True)
    cache.write_text(json.dumps({"key": key, "heat": out}))
    return out


def schedule(inputs: Path, last_day: str) -> dict[int, dict]:
    """Episode dates, premiere and finale per show, from the last inputs file (event timing only: the finale and
    the air dates are on the public schedule ahead of time)."""
    shows = json.loads((inputs / f"{last_day}.json").read_text())["shows"]
    out = {}
    for s in shows:
        air = sorted(set(s.get("air_dates") or []) | set(s.get("schedule") or []))
        fin = None
        if s.get("episodes") and s.get("aired") == s["episodes"] and s.get("air_dates"):
            fin = max(s["air_dates"])
        out[s["id"]] = {"first_air": s.get("first_air"), "air": air, "finale": fin, "long_runner": s.get("long_runner")}
    return out


class World:
    def __init__(self, out: Path, inputs: Path):
        series = json.loads((out / "series.json").read_text())
        comps = {c["id"]: c for c in json.loads((out / "events.json").read_text())["companies"]}
        self.days: list[str] = series["days"]
        self.ix = {d: i for i, d in enumerate(self.days)}
        n = len(self.days)
        self.ids: list[int] = []
        # a row of the replay's paths is one trading day: the value the session trades on (set by the previous
        # close, `Recorder::day_start`) and the dividend that day's close pays (Sunday). So day i's value is
        # "after close i-1", and a holder from day i-1 to day i is paid the dividend of row i-1, which the value
        # of row i is already ex. div[c][i] below is that dividend, shifted onto the day its value drops.
        self.value: dict[int, list[float | None]] = {}   # the value day i trades on (cents/share)
        self.div: dict[int, list[float]] = {}            # dividend received holding from day i-1 to day i
        self.group: dict[int, str] = {}
        for cid, grp, rows in series["paths"]:
            v, dv = [None] * n, [0.0] * n
            for d, _close, fair, div in rows:
                if d in self.ix and fair > 0:
                    v[self.ix[d]] = float(fair)
                    if self.ix[d] + 1 < n:
                        dv[self.ix[d] + 1] = float(div)
            self.ids.append(cid)
            self.value[cid], self.div[cid], self.group[cid] = v, dv, grp
        self.shares = {c: comps[c]["shares"] for c in self.ids if c in comps}
        self.title = {c: comps[c]["title"] for c in self.ids if c in comps}
        self.ipo = {c: (comps[c]["ipo_price"], comps[c]["listed_on"]) for c in self.ids if c in comps}
        self.heat = heat_by_day(inputs, self.days)
        self.sched = schedule(inputs, self.days[-1])

    def alive(self, c: int, i: int) -> bool:
        return 0 <= i < len(self.days) and self.value[c][i] is not None

    def total_return(self, c: int, a: int, b: int) -> float | None:
        """Buy at day a's value, sell at day b's (or the last value, if the stock ends before b: delisting pays the
        book value, which is what the last value is), with the dividends paid in between."""
        v = self.value[c]
        if not self.alive(c, a):
            return None
        end, divs = a, 0.0
        for t in range(a + 1, min(b, len(self.days) - 1) + 1):
            if v[t] is None:
                break
            end, divs = t, divs + self.div[c][t]
        return (v[end] + divs) / v[a] - 1.0

    def market(self, a: int, b: int, universe: list[int]) -> float:
        """Cap-weighted total return of the stocks alive at close a, over (a, b]."""
        num = den = 0.0
        for c in universe:
            r = self.total_return(c, a, b)
            if r is None:
                continue
            w = self.value[c][a] * self.shares.get(c, 1)
            num, den = num + w * r, den + w
        return num / den if den else 0.0


# ---------------------------------------------------------------- strategies
# Each returns a list of trades (stock, buy close index, sell close index). Excess = return - market over the same
# window, so a strategy with no edge averages 0 whatever the market did.

def formation_days(w: World, start: int, step: int, hold: int) -> list[int]:
    return list(range(start, len(w.days) - hold, step))


def top_by(w: World, i: int, universe: list[int], key, n: int, reverse=True) -> list[int]:
    alive = [c for c in universe if w.alive(c, i) and key(w, c, i) is not None]
    return sorted(alive, key=lambda c: key(w, c, i), reverse=reverse)[:n]


def npc_heat(w, c, i):
    return w.heat.get(w.days[i], {}).get(c, (None, None))[0]


def public_heat(w, c, i):
    return w.heat.get(w.days[i], {}).get(c, (None, None))[1]


def value_change(k):
    def f(w, c, i):
        v = w.value[c]
        if i - k < 0 or v[i - k] is None or v[i] is None:
            return None
        return math.log((v[i] + sum(w.div[c][i - k + 1:i + 1])) / v[i - k])
    return f


def cross_section(key, n, hold, step=7, reverse=True):
    def run(w: World, universe: list[int], start: int):
        return [(c, i, i + hold) for i in formation_days(w, start, step, hold) for c in top_by(w, i, universe, key, n, reverse)]
    return run


def tercile_spread(key, hold, step=7):
    """Top third minus bottom third: returned as two legs, +1 and -1."""
    def run(w: World, universe: list[int], start: int):
        out = []
        for i in formation_days(w, start, step, hold):
            alive = sorted((c for c in universe if w.alive(c, i) and key(w, c, i) is not None), key=lambda c: key(w, c, i))
            k = len(alive) // 3
            if k == 0:
                continue
            out += [(c, i, i + hold, -1) for c in alive[:k]] + [(c, i, i + hold, 1) for c in alive[-k:]]
        return out
    return run


def event_trades(when, hold_days):
    """Buy at the close before an event day (per stock, from `when(w, c)` -> list of event days), sell hold_days
    closes after the event (None: hold to the end)."""
    def run(w: World, universe: list[int], start: int):
        out = []
        for c in universe:
            for d in when(w, c):
                if d not in w.ix:
                    continue
                e = w.ix[d]
                a = e - 1
                if a < start or not w.alive(c, a):
                    continue
                b = len(w.days) - 1 if hold_days is None else e + hold_days
                if b > len(w.days) - 1:
                    continue
                out.append((c, a, b))
        return out
    return run


def finale(w, c):
    f = w.sched.get(c, {}).get("finale")
    return [f] if f else []


def premiere(w, c):
    f = w.sched.get(c, {}).get("first_air")
    return [f] if f else []


def air_days(w, c):
    s = w.sched.get(c, {})
    return [] if s.get("long_runner") else [d for d in s.get("air", [])]


def ipo_trades(hold):
    def run(w: World, universe: list[int], start: int):
        out = []
        for c in universe:
            price, on = w.ipo.get(c, (None, None))
            if not price or on not in w.ix or w.ix[on] < start:
                continue
            out.append((c, w.ix[on], w.ix[on] + hold, price))
        return out
    return run


def preair_trades(w: World, universe: list[int], start: int):
    """Buy at the value after the first close once listed, sell at the close before the premiere: nothing about
    the show is observed in between except its pre-air list counts, so the value should not drift."""
    out = []
    for c in universe:
        _, on = w.ipo.get(c, (None, None))
        fa = w.sched.get(c, {}).get("first_air")
        if on not in w.ix or fa not in w.ix:
            continue
        a, b = w.ix[on] + 1, w.ix[fa] - 1
        if a >= start and b - a >= 3 and w.alive(c, a):
            out.append((c, a, b))
    return out


STRATEGIES = {
    # does the value follow popularity with a delay? (hold_hottest; v2-valuation §3.10, §3.12)
    "hottest3_28d": ("最热三支，持有 28 天", cross_section(npc_heat, 3, 28)),
    "heat_spread_28d": ("热度前三分之一 − 后三分之一，28 天", tercile_spread(public_heat, 28)),
    # are the value's revisions predictable from its own past? (v2-valuation §7)
    "rev_mom5_7d": ("5 天估值涨幅前五，持有 7 天", cross_section(value_change(5), 5, 7)),
    "rev_rev1_1d": ("昨天估值跌幅前五，持有 1 天", cross_section(value_change(1), 5, 1, step=1, reverse=False)),
    # calendar and lifecycle (finale_hold, finale_flip, premiere_rush, air_calendar)
    "finale_to_end": ("完结前一天买入，持有到最后", event_trades(finale, None)),
    "finale_flip_8d": ("完结前一天买入，完结后 8 天卖出", event_trades(finale, 8)),
    "premiere_1d": ("首播前一天买入，首播后一天卖出", event_trades(premiere, 1)),
    "air_day_1d": ("每集播出前一天买入，播出后一天卖出", event_trades(air_days, 1)),
    # the IPO price (pre-air prior): bought at the issue price, held four weeks
    "ipo_28d": ("按发行价全额获配，持有 28 天", ipo_trades(28)),
    "preair": ("上市后买入，首播前一天卖出", preair_trades),
}


def evaluate(w: World, trades, universe_set) -> tuple[float, int]:
    """Mean excess return per trade (equal weight), and the number of trades."""
    ex = []
    for t in trades:
        c, a, b = t[0], t[1], t[2]
        if c not in universe_set:
            continue
        if len(t) == 4 and isinstance(t[3], int) and t[3] in (1, -1):        # long/short leg
            r = w.total_return(c, a, b)
            if r is not None:
                ex.append(t[3] * r)
            continue
        if len(t) == 4:                                                        # bought at the IPO price
            r0 = w.total_return(c, a, b)
            if r0 is None:
                continue
            r = (1 + r0) * w.value[c][a] / t[3] - 1
        else:
            r = w.total_return(c, a, b)
            if r is None:
                continue
        ex.append(r - w.market(a, b, w.ids))
    if not ex:
        return float("nan"), 0
    return st.fmean(ex), len(ex)


def spread_mean(w: World, trades, weights: dict[int, int]) -> tuple[float, int]:
    """Long-short legs: per formation day, mean(long) - mean(short), averaged over days. weights: stock -> how many
    times it was drawn (bootstrap)."""
    by_day: dict[int, list[list[float]]] = {}
    for c, a, b, side in trades:
        k = weights.get(c, 0)
        if not k:
            continue
        r = w.total_return(c, a, b)
        if r is None:
            continue
        leg = by_day.setdefault(a, [[], []])[0 if side > 0 else 1]
        leg += [r] * k
    s = [st.fmean(l) - st.fmean(sh) for l, sh in by_day.values() if l and sh]
    return (st.fmean(s) if s else float("nan")), len(s)


def run_strategies(w: World, reps: int, start: int, seed: int = 1) -> dict:
    rng = random.Random(seed)
    out = {}
    for name, (label, fn) in STRATEGIES.items():
        is_spread = name == "heat_spread_28d"
        if is_spread:
            trades = fn(w, w.ids, start)
            point, n = spread_mean(w, trades, {c: 1 for c in w.ids})
        else:
            trades = fn(w, w.ids, start)
            point, n = evaluate(w, trades, set(w.ids))
        # per-stock contributions, so a bootstrap over stocks only re-aggregates
        per_stock: dict[int, list[float]] = {}
        if not is_spread:
            mk: dict[tuple[int, int], float] = {}
            for t in trades:
                c, a, b = t[0], t[1], t[2]
                r = w.total_return(c, a, b)
                if r is None:
                    continue
                if len(t) == 4:
                    r = (1 + r) * w.value[c][a] / t[3] - 1
                if (a, b) not in mk:
                    mk[(a, b)] = w.market(a, b, w.ids)
                per_stock.setdefault(c, []).append(r - mk[(a, b)])
        boots = []
        stocks = w.ids
        for _ in range(reps):
            draw = [stocks[rng.randrange(len(stocks))] for _ in stocks]
            if is_spread:
                cnt: dict[int, int] = {}
                for c in draw:
                    cnt[c] = cnt.get(c, 0) + 1
                m, _ = spread_mean(w, trades, cnt)
            else:
                xs = [x for c in draw for x in per_stock.get(c, [])]
                m = st.fmean(xs) if xs else float("nan")
            if not math.isnan(m):
                boots.append(m)
        boots.sort()
        lo = boots[int(0.05 * len(boots))] if boots else float("nan")
        hi = boots[int(0.95 * len(boots)) - 1] if boots else float("nan")
        top = sorted(((c, sum(xs)) for c, xs in per_stock.items()), key=lambda x: -abs(x[1]))[:2]
        out[name] = {"label": label, "excess": point, "lo": lo, "hi": hi, "n": n,
                     "stocks": len(per_stock) if per_stock else None,
                     "top": [(c, w.title.get(c, str(c))[:12], s) for c, s in top]}
    return out


# ---------------------------------------------------------------- layer-1 diagnostics

def revisions(w: World, start: int) -> dict:
    r_all, pairs = [], []
    for c in w.ids:
        v, prev = w.value[c], None
        for i in range(max(1, start), len(w.days)):
            if v[i] is None or v[i - 1] is None:
                prev = None
                continue
            r = math.log((v[i] + w.div[c][i]) / v[i - 1])
            r_all.append(abs(r))
            if prev is not None:
                pairs.append((prev, r))
            prev = r
    r_all.sort()
    n = len(r_all)
    mx = st.fmean(p for p, _ in pairs)
    my = st.fmean(q for _, q in pairs)
    cov = st.fmean((p - mx) * (q - my) for p, q in pairs)
    vx = st.pvariance([p for p, _ in pairs])
    vy = st.pvariance([q for _, q in pairs])
    return {"n": n, "median": r_all[n // 2], "p90": r_all[int(n * 0.9)], "over5": sum(x > 0.05 for x in r_all) / n,
            "over10": sum(x > 0.10 for x in r_all) / n, "ac1": cov / math.sqrt(vx * vy) if vx and vy else float("nan")}


PHASES = [("preair", "开播前"), ("wk0-3", "开播 0–3 周"), ("wk4+", "开播 4 周后"), ("post", "完结后"), ("long", "长篇连载")]


def phase(w: World, c: int, i: int) -> str:
    s = w.sched.get(c, {})
    if s.get("long_runner"):
        return "long"
    d, fa, fin = w.days[i], s.get("first_air"), s.get("finale")
    if fa and d < fa:
        return "preair"
    if fin and d > fin:
        return "post"
    if fa:
        return "wk0-3" if (date.fromisoformat(d) - date.fromisoformat(fa)).days < 28 else "wk4+"
    return "?"


def martingale(w: World, start: int, reps: int, seed: int = 2) -> dict:
    """The value should be a martingale given what is public: in every phase of a show's life (known in advance)
    the mean daily revision is 0, and today's revision does not predict the next ones. Per phase: the mean daily
    revision, and the slope of the next 1 / 7 days' revisions on today's (> 0: under-reaction, the news is priced
    in over several days; < 0: over-reaction, part of the revision is noise that reverts). Intervals: bootstrap
    over stocks."""
    n = len(w.days)
    rows: dict[str, dict[int, list[tuple[float, float, float]]]] = {}
    for c in w.ids:
        v = w.value[c]
        lr = [None] * n
        for i in range(1, n):
            if v[i] is not None and v[i - 1] is not None:
                lr[i] = math.log((v[i] + w.div[c][i]) / v[i - 1])
        for i in range(max(1, start), n - 7):
            fut = lr[i + 1:i + 8]
            if lr[i] is None or any(x is None for x in fut):
                continue
            rows.setdefault(phase(w, c, i), {}).setdefault(c, []).append((lr[i], fut[0], sum(fut)))

    def stats(xs):
        m = st.fmean(r[0] for r in xs)
        vx = sum((r[0] - m) ** 2 for r in xs)
        sl = []
        for j in (1, 2):
            my = st.fmean(r[j] for r in xs)
            sl.append(sum((r[0] - m) * (r[j] - my) for r in xs) / vx if vx else float("nan"))
        return m, sl[0], sl[1]

    rng = random.Random(seed)
    out = {}
    for g, _ in PHASES:
        by = rows.get(g, {})
        flat = [r for xs in by.values() for r in xs]
        if len(flat) < 30:
            continue
        point = stats(flat)
        ks = list(by)
        bs = [stats([r for _ in ks for r in by[ks[rng.randrange(len(ks))]]]) for _ in range(reps)]
        ci = []
        for j in range(3):
            s = sorted(b[j] for b in bs)
            ci.append((s[int(0.05 * len(s))], s[int(0.95 * len(s)) - 1]))
        out[g] = {"n": len(flat), "stocks": len(ks), "mean": point[0], "slope1": point[1], "slope7": point[2],
                  "ci": ci, "sd": st.pstdev(r[0] for r in flat), "over5": sum(abs(r[0]) > 0.05 for r in flat) / len(flat)}
    return out


def dividend_yield(w: World, start: int) -> dict:
    """Annualised trailing 4-week dividend yield at the last close, by heat group."""
    i = len(w.days) - 1
    by: dict[str, list[float]] = {}
    for c in w.ids:
        v = w.value[c][i]
        if v is None:
            continue
        y = sum(w.div[c][max(start, i - 27):i + 1]) / v * 365 / 28
        by.setdefault(w.group[c], []).append(y)
        by.setdefault("all", []).append(y)
    return {g: st.median(ys) for g, ys in by.items() if ys}


# ---------------------------------------------------------------- report

def pct(x):
    return "   nan" if x is None or (isinstance(x, float) and math.isnan(x)) else f"{x * 100:+6.1f}"


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--world", type=Path, default=FAST / "world.db")
    ap.add_argument("--inputs", type=Path, default=ROOT / "data" / "derived" / "inputs")
    ap.add_argument("--collector", type=Path, default=ROOT / "data" / "anistock.db")
    ap.add_argument("--out", type=Path, default=FAST / "vo")
    ap.add_argument("--no-run", action="store_true", help="reuse the last value-only output in --out")
    ap.add_argument("--set", action="append", default=[], help="marketsim --set key=value (repeatable)")
    ap.add_argument("--reps", type=int, default=400, help="bootstrap draws over stocks")
    ap.add_argument("--start", default=None, help="first formation day (default: 14 days into the replay)")
    ap.add_argument("--save", default=None)
    ap.add_argument("--compare", default=None)
    args = ap.parse_args()

    t0 = time.time()
    if not args.no_run:
        run_value_only(args)
    w = World(args.out, args.inputs)
    start = w.ix[args.start] if args.start else min(14, len(w.days) - 1)
    print(f"{len(w.ids)} stocks, {w.days[0]}..{w.days[-1]}; strategies formed from {w.days[start]}")

    res = run_strategies(w, args.reps, start)
    rev = revisions(w, start)
    dy = dividend_yield(w, start)
    mg = martingale(w, start, max(100, args.reps // 2))
    summary = {"days": [w.days[0], w.days[-1]], "strategies": res, "revisions": rev, "martingale": mg,
               "dividend_yield": dy}

    base = None
    if args.compare:
        p = FAST / f"{args.compare}.json"
        base = json.loads(p.read_text()) if p.exists() else sys.exit(f"{p} not found")

    print("\n机械策略相对市值加权市场的超额收益（每笔，百分点；90% 区间按股票重抽样）")
    print(f"  {'':34} {'超额':>6} {'区间':>15} {'笔数':>6}  {'显著':4}" + ("  对比" if base else "") + "  贡献最大的两支")
    for name, r in res.items():
        sig = "是" if (r["lo"] > 0 or r["hi"] < 0) else ""
        cmp = ""
        if base and name in base["strategies"]:
            cmp = f"  {pct(r['excess'] - base['strategies'][name]['excess'])}"
        tops = "；".join(f"{t}" for _, t, _ in r["top"])
        print(f"  {r['label']:<30} {pct(r['excess'])} [{pct(r['lo'])},{pct(r['hi'])}] {r['n']:6}  {sig:4}{cmp}  {tops}")

    print("\n估值的每日修正（含分红，对数）")
    line = f"  |修正| 中位 {rev['median']:.1%}  P90 {rev['p90']:.1%}  超过 5% {rev['over5']:.0%}  超过 10% {rev['over10']:.0%}  一阶自相关 {rev['ac1']:+.3f}"
    if base:
        b = base["revisions"]
        line += f"\n  对比:  中位 {b['median']:.1%}  P90 {b['p90']:.1%}  超过 5% {b['over5']:.0%}  超过 10% {b['over10']:.0%}  一阶自相关 {b['ac1']:+.3f}"
    print(line)
    print("\n估值是否“只随新信息变化”（按作品阶段；90% 区间按股票重抽样）")
    print("  平均每日修正应为 0；斜率 > 0 是反应不足（消息分几天才反映完），< 0 是反应过度（修正里有会回吐的噪声）")
    print(f"  {'':12} {'平均每日修正':>22}  {'次日斜率':>22}  {'7 天斜率':>22}  {'日波动':>6} {'>5%':>4}")
    for g, label in PHASES:
        m = mg.get(g)
        if not m:
            continue
        cells = []
        for j, (key, scale, fmt) in enumerate((("mean", 100, "{:+.2f}%"), ("slope1", 1, "{:+.2f}"), ("slope7", 1, "{:+.2f}"))):
            lo, hi = m["ci"][j]
            mark = "*" if lo > 0 or hi < 0 else " "
            cells.append(f"{fmt.format(m[key] * scale)} [{fmt.format(lo * scale)},{fmt.format(hi * scale)}]{mark}")
        cmp = ""
        if base and g in base.get("martingale", {}):
            b = base["martingale"][g]
            cmp = f"  对比 {b['mean'] * 100:+.2f}% {b['slope1']:+.2f} {b['slope7']:+.2f}"
        print(f"  {label:<10} " + "  ".join(f"{c:>22}" for c in cells) + f"  {m['sd']:6.1%} {m['over5']:4.0%}{cmp}")
    print("  * 显著")

    print("\n股息率（近 4 周，年化，中位数）  " + "  ".join(f"{g} {y:.0%}" for g, y in sorted(dy.items())))

    if args.save:
        FAST.mkdir(parents=True, exist_ok=True)
        (FAST / f"{args.save}.json").write_text(json.dumps(summary, ensure_ascii=False, indent=1))
        print(f"\nsaved data/fast/{args.save}.json")
    print(f"\n{time.time() - t0:.1f}s")


if __name__ == "__main__":
    main()
