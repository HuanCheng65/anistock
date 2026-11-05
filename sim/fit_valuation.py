"""Fit the valuation filter (engine/valuation.py, docs/v2-valuation.md §3.2) to a season of daily inputs.

    .venv/bin/python sim/fit_valuation.py INPUTS_DIR [--out sim/calibration/valuation_filter.json]
                                         [--report REPORT.md] [--comments COMMENTS.json]

Model, per show and market m (us = AniList, cn = Bangumi, jp = Annict):
    log(attention_t / curve_m(days since the last episode)) = level_t + noise,   noise ~ N(0, r_m)
    level_{t+1} = level_t + drift_m + w,                                       w ~ N(0, q_m)
    level at the premiere ~ N(log(pre-air expectation_m) + prior_bias_m, p0_m)
cn and jp arrive as weekly series (one new value per episode), us daily. The within-episode curve of the us
market is the median shape of complete episode weeks. drift, q, r, p0, prior_bias are fitted per market by maximum
likelihood (Nelder-Mead on the filter's one-step-ahead predictive likelihood; innovations clipped at 4
standard deviations, as in the filter). Validation: the share of observations inside the filter's 80%
prediction interval for the next observation (the next episode's heat for cn/jp, the next week's average
for us), in-sample and on held-out shows (fit on even AniList ids, test on odd).

With --comments (episode comment timestamps from sim/fetch_comments.py), also fits the hourly 热度快讯 curve:
the share of an episode's 28-day comments posted within h hours of airing, and the noise of
log(count_h / (curve_h × previous episodes' mean)) per hour.
"""

from __future__ import annotations

import argparse
import json
import math
import statistics as st
import sys
from datetime import date
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from engine import valuation as V  # noqa: E402

MARKETS = V.MARKETS
CLIP = 4.0
Z80 = 1.2815515655446004


def load(inputs_dir: Path):
    files = sorted(inputs_dir.glob("*.json"))
    shows: dict[int, dict] = {}
    for f in files:
        d = json.loads(f.read_text(encoding="utf-8"))
        day = date.fromisoformat(d["day"]).toordinal()
        for s in d["shows"]:
            x = shows.setdefault(s["id"], {"days": [], "att": {m: [] for m in MARKETS}, "exp": {m: [] for m in MARKETS},
                                           "pre": {m: [] for m in MARKETS},
                                           "aired": [], "first_air": date.fromisoformat(s["first_air"]).toordinal(),
                                           "long_runner": s["long_runner"]})
            x["days"].append(day)
            x["aired"].append(s["aired"])
            for m in MARKETS:
                x["att"][m].append(s["att"][m])
                x["exp"][m].append(s["expected"][m])
                x["pre"][m].append((s.get("preair") or s["expected"])[m])
    return shows


def air_days(x) -> list[int]:
    out, prev = [], 0
    for d, a in zip(x["days"], x["aired"]):
        if a > prev:
            out.append(d)
        prev = a
    return out


def fit_curve(shows) -> list[float]:
    ratios = [[] for _ in range(7)]
    for x in shows.values():
        if x["long_runner"]:
            continue
        idx = {d: i for i, d in enumerate(x["days"])}
        ad = air_days(x)
        for a, b in zip(ad, ad[1:]):
            if b - a != 7 or not all((a + k) in idx for k in range(7)):
                continue
            v = [x["att"]["us"][idx[a + k]] for k in range(7)]
            mu = sum(v) / 7
            if mu <= 0:
                continue
            for k in range(7):
                ratios[k].append(v[k] / mu)
    c = [st.median(r) for r in ratios]
    s = sum(c) / 7
    return [round(v / s, 4) for v in c]


def observations(x, m, curve_us):
    """[(day, y, kind)] for market m: kind 'obs'. Plus the prior (log pre-air expectation) at the premiere."""
    ad = air_days(x)
    if not ad or x["long_runner"]:
        return None, []
    prem = ad[0]
    idx = {d: i for i, d in enumerate(x["days"])}
    # the prior is the pre-air estimate on the premiere day (what the server's filter starts from)
    pre_i = idx.get(prem)
    prior = x["pre"][m][pre_i] if pre_i is not None else 0.0
    if x["aired"][0] > 0:
        # premiered before the data starts: its first day here is not a premiere, and the pre-air estimate
        # on that day is not a prior (the filter starts from the first observation instead)
        prior = 0.0
    out = []
    prev = None
    acc, acc_n = 0.0, 0
    for i, d in enumerate(x["days"]):
        if d < prem:
            continue
        v = x["att"][m][i]
        if v <= 0:
            continue
        if m == "us":
            # daily values are averaged over each episode week and observed once, when the next episode
            # airs (or after 7 days): within-week deviations from the average curve are not information
            a = V.last_air_before(ad, d)
            k = None if a is None else d - a
            c = curve_us[k] if (k is not None and k < 7) else 1.0
            if acc_n and (k == 0 or acc_n >= 7):
                out.append((d, acc / acc_n, acc_n))
                acc, acc_n = 0.0, 0
            acc += math.log(v / c)
            acc_n += 1
        else:
            if prev is not None and v == prev:
                continue
            prev = v
            out.append((d, math.log(v), 1))
    return (math.log(prior) if prior > 0 else None), out


def run_filter(prior, obs, prem_day, prm):
    """Log-likelihood and the one-step predictions [(y, mean_pred, var_pred)]."""
    drift, q, r, p0, bias, pi, r_out = prm
    if not obs:
        return 0.0, []
    if prior is None:
        mean, var = V.week_observation(obs[0][1], obs[0][2], drift), r
        day, rest = obs[0][0], obs[1:]
    else:
        mean, var, day, rest = prior + bias, p0, prem_day, obs
    ll = 0.0
    preds = []
    for d, y_mean, n in rest:
        y = V.week_observation(y_mean, n, drift)
        mean, var = V.kf_predict(mean, var, d - day, drift, q)
        day = d
        preds.append((y, mean, (var + r, var + r + r_out, pi)))
        mean, var, l = V.robust_update(mean, var, y, r, pi, r_out, CLIP)
        ll += l
    return ll, preds


def level_bias(data, prm):
    """The prior bias that makes the first revision of the value zero on average (in level, not log).

    The value is exp(mean + var/2) of the level. Its expected change at the first observation is zero when the
    model is exactly right; the maximum-likelihood bias is unbiased in log, and the prior errors are not
    exactly lognormal, so the mean first revision of the value would be predictable (negative). This picks the
    bias b with mean_shows exp(K (y - m0(b)) + (v1 - v0) / 2) = 1, by bisection."""
    drift, q, r, p0, bias, pi, r_out = prm
    firsts = []
    for prior, obs, prem in data:
        if prior is None or not obs:
            continue
        d, y_mean, n = obs[0]
        y = V.week_observation(y_mean, n, drift)
        m0, v0 = V.kf_predict(prior, p0, d - prem, drift, q)
        k = v0 / (v0 + r)
        firsts.append((y - m0, k, v0 * (1 - k) - v0))
    if not firsts:
        return bias

    def f(b):
        return math.log(sum(math.exp(k * (e - b) + 0.5 * dv) for e, k, dv in firsts) / len(firsts))

    lo, hi = -3.0, 3.0
    for _ in range(100):
        mid = 0.5 * (lo + hi)
        if f(mid) > 0:
            lo = mid
        else:
            hi = mid
    return 0.5 * (lo + hi)


def total_ll(data, prm):
    return sum(run_filter(prior, obs, prem, prm)[0] for prior, obs, prem in data)


def nelder_mead(f, x0, step, iters=600):
    n = len(x0)
    pts = [list(x0)] + [[x0[j] + (step[j] if j == i else 0) for j in range(n)] for i in range(n)]
    vals = [f(p) for p in pts]
    for _ in range(iters):
        order = sorted(range(n + 1), key=lambda i: vals[i])
        pts, vals = [pts[i] for i in order], [vals[i] for i in order]
        c = [sum(p[j] for p in pts[:-1]) / n for j in range(n)]
        xr = [c[j] + (c[j] - pts[-1][j]) for j in range(n)]
        fr = f(xr)
        if fr < vals[0]:
            xe = [c[j] + 2 * (c[j] - pts[-1][j]) for j in range(n)]
            fe = f(xe)
            pts[-1], vals[-1] = (xe, fe) if fe < fr else (xr, fr)
        elif fr < vals[-2]:
            pts[-1], vals[-1] = xr, fr
        else:
            xc = [c[j] + 0.5 * (pts[-1][j] - c[j]) for j in range(n)]
            fc = f(xc)
            if fc < vals[-1]:
                pts[-1], vals[-1] = xc, fc
            else:
                pts = [pts[0]] + [[pts[0][j] + 0.5 * (p[j] - pts[0][j]) for j in range(n)] for p in pts[1:]]
                vals = [vals[0]] + [f(p) for p in pts[1:]]
        if max(vals) - min(vals) < 1e-7:
            break
    i = min(range(n + 1), key=lambda i: vals[i])
    return pts[i], vals[i]


GAUSSIAN = False


def to_prm(x):
    # the observation noise is floored at 1e-4: cn/jp are rebuilt series and can look noise-free in-sample
    pi = 0.0 if GAUSSIAN else 1 / (1 + math.exp(-x[5]))
    return (x[0], math.exp(x[1]), max(1e-4, math.exp(x[2])), math.exp(x[3]), x[4], pi, math.exp(x[6]))


def mixture_mass(a, s1, s2, pi):
    """P(|V| <= a) for V ~ (1-pi) N(0, s1) + pi N(0, s2)."""
    return (1 - pi) * math.erf(a / math.sqrt(2 * s1)) + pi * math.erf(a / math.sqrt(2 * s2))


def coverage(data, prm, market):
    """Share of next observations (the next episode's value; us: the next episode week's average) inside
    the filter's central 80% prediction interval (of the predictive mixture)."""
    inside = total = 0
    for prior, obs, prem in data:
        _, preds = run_filter(prior, obs, prem, prm)
        for y, mu, (s1, s2, pi) in preds:
            total += 1
            inside += mixture_mass(abs(y - mu), s1, s2, pi) <= 0.8
    return inside / max(1, total), total


def fit_flash(path: Path):
    """Hourly curve and noise from comment timestamps: {episode_id: {"air": unix, "times": [unix...],
    "prev_final": [counts]}}."""
    data = json.loads(path.read_text(encoding="utf-8"))
    hours = 24
    frac = [[] for _ in range(hours)]
    for e in data.values():
        times = [t - e["air"] for t in e["times"] if t >= e["air"]]
        n28 = sum(1 for t in times if t <= 28 * 86400)
        if n28 < 20:
            continue
        for h in range(hours):
            frac[h].append(sum(1 for t in times if t <= (h + 1) * 3600) / n28)
    curve = [round(st.median(f), 4) if f else 0.0 for f in frac]
    noise = []
    for h in range(hours):
        z = []
        for e in data.values():
            prev = e.get("prev_final") or []
            if not prev or curve[h] <= 0:
                continue
            mp = sum(prev) / len(prev)
            cnt = sum(1 for t in e["times"] if e["air"] <= t <= e["air"] + (h + 1) * 3600)
            if mp * curve[h] >= 5 and cnt > 0:
                z.append(math.log(cnt / (mp * curve[h])))
        noise.append(round(st.pvariance(z), 4) if len(z) > 5 else 1.0)
    return curve, noise, len([1 for f in frac[0]])


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("inputs", type=Path)
    ap.add_argument("--out", type=Path, default=ROOT / "sim" / "calibration" / "valuation_filter.json")
    ap.add_argument("--report", type=Path)
    ap.add_argument("--comments", type=Path)
    ap.add_argument("--gaussian", action="store_true", help="no outlier component (a plain Kalman filter)")
    ap.add_argument("--clip", type=float, default=4.0)
    a = ap.parse_args()
    global GAUSSIAN, CLIP
    GAUSSIAN, CLIP = a.gaussian, a.clip
    shows = load(a.inputs)
    curve_us = fit_curve(shows)
    res = {"curve_us": curve_us, "drift": {}, "q": {}, "r": {}, "p0": {}, "prior_bias": {}, "outlier_p": {},
           "outlier_r": {}, "fit": {}}
    lines = [f"# Valuation filter fit\n", f"Data: {a.inputs} ({len(shows)} shows).\n",
             f"us within-episode curve (day 0..6 after airing, mean 1): {curve_us}\n"]
    if GAUSSIAN:
        lines.append("Plain Kalman filter (`--gaussian`, no outlier component): the outlier mixture was also fitted and "
                     "gave no better held-out coverage, so the simpler model is used.\n")
    for m in MARKETS:
        data = []
        for sid, x in sorted(shows.items()):
            prior, obs = observations(x, m, curve_us)
            ad = air_days(x)
            if obs and len(obs) >= 3:
                data.append((prior, obs, ad[0], sid))
        all_data = [(p, o, d) for p, o, d, _ in data]
        x0 = [-0.003, math.log(0.002), math.log(0.05), math.log(0.5), 0.0, -2.0, math.log(0.5)]
        steps = [0.002, 1.0, 1.0, 1.0, 0.3, 1.0, 1.0]
        best, nll = nelder_mead(lambda x: -total_ll(all_data, to_prm(x)), x0, steps, iters=3000)
        prm = to_prm(best)
        cov, n = coverage(all_data, prm, m)
        even = [(p, o, d) for p, o, d, s in data if s % 2 == 0]
        odd = [(p, o, d) for p, o, d, s in data if s % 2 == 1]
        be, _ = nelder_mead(lambda x: -total_ll(even, to_prm(x)), x0, steps, iters=3000)
        cov_odd, n_odd = coverage(odd, to_prm(be), m)
        (res["drift"][m], res["q"][m], res["r"][m], res["p0"][m], res["prior_bias"][m], res["outlier_p"][m],
         res["outlier_r"][m]) = (round(v, 6) for v in prm)
        # the server uses the level-unbiased prior bias (the first revision of the value is zero on average)
        lb = level_bias(all_data, prm)
        res["prior_bias"][m] = round(lb, 6)
        res["fit"][m] = {"shows": len(data), "observations": n, "loglik": -nll, "coverage80_in_sample": cov,
                         "coverage80_held_out": cov_odd, "held_out_observations": n_odd,
                         "prior_bias_mle": round(prm[4], 6), "prior_bias_level": round(lb, 6)}
        lines.append(f"## {m}\n\nshows {len(data)}, observations {n}; drift {prm[0]:.5f}/day "
                     f"(×{math.exp(7 * prm[0]):.3f}/week), q {prm[1]:.5f}/day, r {prm[2]:.4f}, p0 {prm[3]:.3f}, "
                     f"prior bias {prm[4]:+.3f}, outliers {prm[5]:.1%} with extra variance {prm[6]:.3f}; "
                     f"log-likelihood {-nll:.1f}.\n\n80% interval coverage: {cov:.1%} in sample, {cov_odd:.1%} "
                     f"on held-out shows ({n_odd} observations; fit on even ids, tested on odd).\n\n"
                     f"Prior bias used by the server: {lb:+.3f} (maximum likelihood {prm[4]:+.3f} is unbiased in log; "
                     f"this one makes the value's first revision after the premiere zero on average).\n")
    if a.comments:
        curve, noise, n = fit_flash(a.comments)
        res["flash_curve"], res["flash_r"] = curve, noise
        lines.append(f"## 热度快讯 hourly curve\n\n{n} episodes. Share of 28-day comments by hour 1..24: {curve}\n\n"
                     f"Noise variance of log(count_h / (curve_h × previous mean)) by hour: {noise}\n")
    res["clip"] = CLIP
    a.out.parent.mkdir(parents=True, exist_ok=True)
    a.out.write_text(json.dumps(res, ensure_ascii=False, indent=1), encoding="utf-8")
    if a.report:
        a.report.write_text("\n".join(lines), encoding="utf-8")
    print("\n".join(lines))


if __name__ == "__main__":
    main()
