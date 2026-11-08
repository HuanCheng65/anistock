"""Fit the sequel option (docs/design/series.md §5) on the history backfill (sim/history_backfill.py).

    python sim/fit_sequel.py HISTORY_DB [--out sim/calibration/sequel.json] [--report REPORT.md]

Model. A finished show's next season is announced, if at all, at the finale (the finale's day ± a week: period 0) or
later, with a yearly hazard (periods 1, 2, 3; then one hazard for every later year, period 4). A discrete-time
logistic hazard: P(announced in period k | not before) = σ(α_k + β·x), x = (score − 70)/10, light-novel source,
original, itself a sequel, top decile of popularity among the shows finishing that year. Every variable is public at
the finale. Fitted by maximum likelihood (Newton) on the TV/ONA shows that finished 2018-2025; a period counts only
if it has fully elapsed (censoring). The announcement is the first AniList trend day of a TV/ONA/film sequel entry,
else its start date (sim/calibration/long_tail_sequel.md).

Then: the lag from announcement to premiere (TV/ONA sequels: ten quantiles), and the sequel's in-season us attention
relative to its prequel's (deflated by the platform index) by the gap from the prequel's finale to the sequel's
premiere: the expected (lognormal mean) ratio per gap bucket. Calibration: the share announced within 3 years,
predicted vs actual, by predicted decile; held out (fit on even ids, test on odd).
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from datetime import date, timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from sim.tail_history import SERIES, History, fdate  # noqa: E402

PERIODS = [(-36500, 7), (7, 372), (372, 737), (737, 1102), (1102, 1467), (1467, 1832), (1832, 2197), (2197, 2562)]
# period index for the intercept: 0 at the finale, 1-3 the first three years, 4 every later year
PERIOD_ALPHA = [0, 1, 2, 3, 4, 4, 4, 4]
NAMES = ["score/10", "light novel", "original", "itself a sequel", "top decile popularity"]


def sig(z):
    return 1 / (1 + math.exp(-z)) if z > -700 else 0.0


def rows_of(H: History):
    """[(id, x, announcement day relative to the finale or None, days observed since the finale)]."""
    shows = []
    for mid, m in H.media.items():
        e = fdate(m.get("endDate"))
        if m["status"] != "FINISHED" or not e or m["format"] not in SERIES:
            continue
        a = H.announced(m)
        pop = m.get("popularity") or 0
        t = H.trend.get(mid, {})
        for k in range(14, 45):
            d = e + timedelta(days=k)
            if d in t:
                pop = t[d][1] or pop
                break
        shows.append(dict(id=mid, e=e, m=m, ann=(a - e).days if a else None, obs=(H.today - e).days, pop=pop))
    by_y = {}
    for s in shows:
        by_y.setdefault(s["e"].year, []).append(s["pop"])
    out = []
    for s in shows:
        m = s["m"]
        v = by_y[s["e"].year]
        pct = sum(1 for p in v if p < s["pop"]) / len(v)
        src = m.get("source") or "OTHER"
        prequel = any(ed["relationType"] == "PREQUEL" and ed["node"]["type"] == "ANIME"
                      and ed["node"]["format"] in SERIES | {"MOVIE"} for ed in m["relations"]["edges"])
        x = [((m.get("averageScore") or 65) - 70) / 10, float(src == "LIGHT_NOVEL"), float(src == "ORIGINAL"),
             float(prequel), float(pct >= 0.9)]
        out.append((s["id"], x, s["ann"], s["obs"]))
    return out


def person_periods(rows):
    """Discrete-time hazard rows: (alpha index, x, event)."""
    pp = []
    for _, x, ann, obs in rows:
        for k, (lo, hi) in enumerate(PERIODS):
            if obs < hi:
                break   # the period has not fully elapsed: censored
            if ann is not None and ann < lo:
                break   # announced before this period
            ev = ann is not None and lo <= ann < hi
            pp.append((PERIOD_ALPHA[k], x, 1.0 if ev else 0.0))
            if ev:
                break
    return pp


def fit(pp, iters=50):
    na, nx = 5, len(pp[0][1])
    n = na + nx
    th = [0.0] * n

    def design(a, x):
        return [1.0 if i == a else 0.0 for i in range(na)] + list(x)
    for _ in range(iters):
        g = [0.0] * n
        H = [[0.0] * n for _ in range(n)]
        for a, x, y in pp:
            d = design(a, x)
            p = sig(sum(t * v for t, v in zip(th, d)))
            w = p * (1 - p)
            for i in range(n):
                g[i] += (y - p) * d[i]
                if d[i]:
                    for j in range(n):
                        H[i][j] += w * d[i] * d[j]
        step = solve(H, g)
        th = [t + s for t, s in zip(th, step)]
        if max(abs(s) for s in step) < 1e-9:
            break
    se = [math.sqrt(max(0.0, v)) for v in diag_inv(H)]
    return th[:na], th[na:], se


def solve(A, b):
    n = len(b)
    M = [list(r) + [b[i]] for i, r in enumerate(A)]
    for i in range(n):
        p = max(range(i, n), key=lambda r: abs(M[r][i]))
        M[i], M[p] = M[p], M[i]
        for r in range(n):
            if r != i and M[r][i]:
                f = M[r][i] / M[i][i]
                for c in range(i, n + 1):
                    M[r][c] -= f * M[i][c]
    return [M[i][n] / M[i][i] for i in range(n)]


def diag_inv(A):
    n = len(A)
    return [solve(A, [1.0 if j == i else 0.0 for j in range(n)])[i] for i in range(n)]


def p_within(alpha, beta, x, years=3):
    """P(announced within `years` of the finale, at the finale included)."""
    z = sum(b * v for b, v in zip(beta, x))
    surv = 1.0
    for k in range(years + 1):
        surv *= 1 - sig(alpha[min(k, 4)] + z)
    return 1 - surv


def calibration(rows, alpha, beta, years=3):
    el = [(x, ann) for _, x, ann, obs in rows if obs >= 365 * years + 7]
    pr = sorted((p_within(alpha, beta, x, years), ann is not None and ann < 365 * years + 7) for x, ann in el)
    out = []
    k = 5
    for i in range(k):
        g = pr[i * len(pr) // k:(i + 1) * len(pr) // k]
        out.append((sum(p for p, _ in g) / len(g), sum(1 for _, y in g if y) / len(g), len(g)))
    return out


def lag_quantiles(H: History):
    lags = []
    for mid, m in H.media.items():
        for n in H.sequel_nodes(m):
            if n["format"] not in SERIES:
                continue
            sd = fdate(n.get("startDate"))
            a = H.first_trend.get(n["id"])
            if sd and a and sd <= H.today and sd >= a:
                lags.append((sd - a).days)
    lags = sorted(set(lags)) if False else sorted(lags)
    return [lags[int((i + 0.5) / 10 * len(lags))] for i in range(10)], len(lags)


def gap_ratios(H: History):
    """Sequel in-season us attention / prequel's, both deflated by the platform index, by gap (days from the
    prequel's finale to the sequel's premiere): per bucket the lognormal mean ratio, n, geometric mean."""
    buckets = [(0, 365), (365, 730), (730, 1460), (1460, 100000)]
    vals = {b: [] for b in buckets}
    for mid, m in H.media.items():
        s, e = fdate(m.get("startDate")), fdate(m.get("endDate"))
        if not s or not e or m["status"] != "FINISHED" or m["format"] not in SERIES or (e - s).days > 400:
            continue
        a0 = H.window_att(mid, s, e)
        if a0 <= 0:
            continue
        for n in H.sequel_nodes(m):
            if n["format"] not in SERIES or n["id"] not in H.media:
                continue
            m2 = H.media[n["id"]]
            s2, e2 = fdate(m2.get("startDate")), fdate(m2.get("endDate"))
            if not s2 or not e2 or m2["status"] != "FINISHED" or (e2 - s2).days > 400 or s2 <= e:
                continue
            a1 = H.window_att(n["id"], s2, e2)
            if a1 <= 0:
                continue
            r = a1 / a0 * H.index_at(s + (e - s) / 2) / H.index_at(s2 + (e2 - s2) / 2)
            gap = (s2 - e).days
            for b in buckets:
                if b[0] <= gap < b[1]:
                    vals[b].append(math.log(r))
    out = []
    for b in buckets:
        v = vals[b]
        mu = sum(v) / len(v)
        var = sum((y - mu) ** 2 for y in v) / len(v)
        out.append({"until_days": b[1], "ratio": round(math.exp(mu + 0.5 * var), 6), "n": len(v),
                    "geo": round(math.exp(mu), 4), "log_sd": round(math.sqrt(var), 4)})
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("history", type=Path)
    ap.add_argument("--out", type=Path, default=ROOT / "sim" / "calibration" / "sequel.json")
    ap.add_argument("--report", type=Path)
    ap.add_argument("--cn-conv", type=float, default=1.076,
                    help="cn in-season mean / finale week (sim/calibration/valuation_filter.md, Long tail, cn)")
    a = ap.parse_args()
    H = History(str(a.history))
    rows = rows_of(H)
    pp = person_periods(rows)
    alpha, beta, se = fit(pp)
    ev = person_periods([r for r in rows if r[0] % 2 == 0])
    a_e, b_e, _ = fit(ev)
    cal_in = calibration(rows, alpha, beta)
    cal_out = calibration([r for r in rows if r[0] % 2 == 1], a_e, b_e)
    lags, nlag = lag_quantiles(H)
    gaps = gap_ratios(H)
    # a season's finale week relative to its in-season mean attention (the sequel's tail starts there): us from the
    # history (with the filter's finale bump), cn the inverse of the tail fit's in-season/finale-week ratio, jp as us
    from sim.fit_valuation import hist_in_season_ratio
    fpj = json.loads((ROOT / "sim" / "calibration" / "valuation_filter.json").read_text(encoding="utf-8"))
    c_us = hist_in_season_ratio(H, fpj["finale_bump"]["us"])
    s2f = {"us": round(c_us, 6), "cn": round(1 / a.cn_conv, 6), "jp": round(c_us, 6)}
    res = {"season_to_finale": s2f,"alpha": [round(v, 6) for v in alpha], "beta": dict(zip(["score", "light_novel", "original", "sequel",
                                                                    "top_pop"], [round(v, 6) for v in beta])),
           "period_days": [7, 372, 737, 1102, 365], "lag_days": lags, "gap_ratio": gaps,
           "transient": {"week1": 1.286, "weeks2_3": 1.058}}
    lines = ["# Sequel option fit (docs/design/series.md §5)\n", f"Data: {a.history}; {len(rows)} finished TV/ONA shows, "
             f"{len(pp)} show-periods.\n", "Discrete-time logistic hazard: period 0 = the finale ± 7 days, periods 1–3 = "
             "years 1–3 after it, period 4 = each later year.\n",
             "| period | α | P for a first season, score 70, other source, not top decile |\n|---|---|---|"]
    for k in range(5):
        lines.append(f"| {k} | {alpha[k]:+.3f} | {sig(alpha[k]):.1%} |")
    lines.append("\n| predictor | β | se | odds × |\n|---|---|---|---|")
    for nm, b, s in zip(NAMES, beta, se[5:]):
        lines.append(f"| {nm} | {b:+.3f} | {s:.3f} | {math.exp(b):.2f} |")
    lines.append("\nCalibration, announced within 3 years (predicted vs actual by fifth of the predicted probability; "
                 "in sample | fit on even ids, tested on odd):\n\n| fifth | predicted | actual | n | predicted (held out) "
                 "| actual (held out) | n |\n|---|---|---|---|---|---|---|")
    for i, (x, y) in enumerate(zip(cal_in, cal_out)):
        lines.append(f"| {i + 1} | {x[0]:.1%} | {x[1]:.1%} | {x[2]} | {y[0]:.1%} | {y[1]:.1%} | {y[2]} |")
    lines.append(f"\nAnnouncement → premiere (TV/ONA, {nlag} sequels), decile midpoints: {lags} days.\n")
    lines.append("Sequel in-season us attention / prequel's (deflated), by gap from the prequel's finale:\n\n"
                 "| gap up to (days) | n | geometric mean | log sd | expected ratio |\n|---|---|---|---|---|")
    for g in gaps:
        lines.append(f"| {g['until_days']} | {g['n']} | {g['geo']} | {g['log_sd']} | {g['ratio']} |")
    lines.append(f"\nFinale week relative to the in-season mean (where a sequel season's tail starts): {s2f}.\n")
    lines.append("\nAnnouncement transient on the prequel's us attention (sim/calibration/long_tail_sequel.md, 306 "
                 "announcements): ×1.286 in the announcement week, ×1.058 in the two weeks after, then none. The "
                 "filter reads those weeks' observations through it.\n")
    a.out.write_text(json.dumps(res, ensure_ascii=False, indent=1), encoding="utf-8")
    text = "\n".join(lines)
    if a.report:
        a.report.write_text(text, encoding="utf-8")
    print(text)


if __name__ == "__main__":
    main()
