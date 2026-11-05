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
                                           "pre": {m: [] for m in MARKETS}, "obs": {m: [] for m in MARKETS},
                                           "prior_us": [], "src": {m: True for m in MARKETS},
                                           "aired": [], "first_air": date.fromisoformat(s["first_air"]).toordinal(),
                                           "long_runner": s["long_runner"], "episodes": s.get("episodes")})
            x["days"].append(day)
            x["aired"].append(s["aired"])
            # v0.3.2 inputs: the point-in-time prior (us; cn/jp derive from it) and which markets reported
            pr = s.get("prior")
            x["prior_us"].append(pr["us"] if pr else None)
            if s.get("sources"):
                x["src"] = {m: bool(s["sources"].get(m, True)) for m in MARKETS}
            for m in MARKETS:
                x["att"][m].append(s["att"][m])
                x["exp"][m].append(s["expected"][m])
                x["pre"][m].append((s.get("preair") or s["expected"])[m])
                ob = s.get("observed")
                x["obs"][m].append(ob[m] if ob else s["att"][m] > 0)
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


def finale_day(x):
    """The day of a show's last episode if it has finished: its announced count aired, or nothing new for two
    weeks before the end of the data (a season released at once, as the server reads it)."""
    n = x.get("episodes")
    ad = air_days(x)
    if x["long_runner"] or not n or not ad:
        return None
    if len(ad) == n:
        return ad[n - 1]
    if len(ad) < n and x["days"][-1] - ad[-1] >= 14:
        return ad[-1]
    return None


BUMP = {"us": 1.0, "cn": 1.0, "jp": 1.0}


def fit_finale_bump(shows):
    """Per market, attention in the finale's week relative to the level two to three weeks before it (finished
    shows with 10+ episodes; AniList counts completions in the finale week)."""
    out = {}
    for m in MARKETS:
        r0, ref = [], []
        for x in shows.values():
            fin = finale_day(x)
            if fin is None or x["aired"][0] > 0 or len(air_days(x)) < 10:
                continue
            idx = {d: i for i, d in enumerate(x["days"])}

            def wk(lo):
                v = [x["att"][m][idx[d]] for d in range(lo, lo + 7) if d in idx and x["obs"][m][idx[d]]]
                return sum(v) / len(v) if len(v) >= 4 else None
            a, b, c = wk(fin), wk(fin - 14), wk(fin - 21)
            if a and b and c and b > 0 and c > 0:
                r0.append(a / ((b + c) / 2))
        out[m] = round(sum(r0) / len(r0), 6) if len(r0) >= 20 else 1.0
    return out


def observations(x, m, curve_us):
    """[(day, y, kind)] for market m: kind 'obs'. Plus the prior (log pre-air expectation) at the premiere."""
    ad = air_days(x)
    if not ad or x["long_runner"]:
        return None, []
    prem = ad[0]
    idx = {d: i for i, d in enumerate(x["days"])}
    # the prior is the pre-air estimate on the premiere day (what the server's filter starts from)
    pre_i = idx.get(prem)
    if pre_i is not None and x["prior_us"][pre_i] is not None:
        # point-in-time prior: the us pre-air estimate for every market (prior_bias_m converts it)
        prior = x["prior_us"][pre_i]
    else:
        prior = x["pre"][m][pre_i] if pre_i is not None else 0.0
    if x["aired"][0] > 0:
        # premiered before the data starts: its first day here is not a premiere, and the pre-air estimate
        # on that day is not a prior (the filter starts from the first observation instead)
        prior = 0.0
    # in season only: from a week after the finale the level is observed through the post-finale profile and
    # no longer drifts (the server does the same), so those days say nothing about the in-season drift
    fin = finale_day(x)
    out = []
    prev = None
    acc, acc_n = 0.0, 0
    for i, d in enumerate(x["days"]):
        if d < prem or (fin is not None and d >= fin + 7):
            continue
        v = x["att"][m][i]
        if v <= 0 or not x["obs"][m][i]:
            continue
        if fin is not None and fin <= d < fin + 7:
            v = v / BUMP[m]   # the finale's week, read through its fitted bump (as the server does)
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


def d_at(dr, prem, t):
    """Daily drift on day t of a show premiering on prem: dr = (weeks 0-3, 4-7, 8+)."""
    w = (t - prem) // 7
    return dr[0] if w < V.DRIFT_WEEKS[0] else (dr[1] if w < V.DRIFT_WEEKS[1] else dr[2])


def d_cum(dr, prem, t0, t1):
    return sum(d_at(dr, prem, t) for t in range(t0, t1))


def run_filter(prior, obs, prem_day, prm):
    """Log-likelihood and the one-step predictions [(y, mean_pred, var_pred)]."""
    drift, q, r, p0, bias, pi, r_out = prm
    if not obs:
        return 0.0, []
    if prior is None:
        mean, var = V.week_observation(obs[0][1], obs[0][2], d_at(drift, prem_day, obs[0][0])), r
        day, rest = obs[0][0], obs[1:]
    else:
        mean, var, day, rest = prior + bias, p0, prem_day, obs
    ll = 0.0
    preds = []
    for d, y_mean, n in rest:
        y = V.week_observation(y_mean, n, d_at(drift, prem_day, d))
        mean, var = mean + d_cum(drift, prem_day, day, d), var + q * (d - day)
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


def preair_growth(shows, kmax=35):
    """How the point-in-time pre-air estimate grows until it freezes the day before the premiere (AniList list
    entries keep coming in): for k days before the freeze, the mean and variance over shows of
    log(estimate at the freeze / estimate k days earlier). The server adds them to the prior of a show not
    yet airing, so its value does not rise predictably while it waits for the premiere."""
    by = {k: [] for k in range(kmax + 1)}
    for x in shows.values():
        ad = air_days(x)
        if not ad or x["long_runner"] or x["aired"][0] > 0:
            continue
        prem = ad[0]
        pts = {d: p for d, p in zip(x["days"], x["prior_us"]) if p and d < prem}
        if (prem - 1) not in pts:
            continue
        end = pts[prem - 1]
        for d, p in pts.items():
            k = prem - 1 - d
            if 0 <= k <= kmax and p > 0:
                by[k].append(math.log(end / p))
    out, last = [], [0.0, 0.0]
    for k in range(kmax + 1):
        v = by[k]
        if len(v) >= 10:
            mu = sum(v) / len(v)
            last = [round(mu, 6), round(sum((x - mu) ** 2 for x in v) / len(v), 6)]
        out.append(list(last))
    return out, {k: len(v) for k, v in by.items()}


def post_finale(shows, drift, weeks=6, min_shows=20):
    """Per market, the attention after the finale week relative to the level at the end of the finale week
    (what the revenue engine will see: us observed, cn/jp the rebuilt series), for weeks 0..: the mean over
    finished shows of (mean attention of the week) / (mean attention of the finale week × exp(3.5 drift)),
    the finale week's mean describing its middle. The tail: the mean week-on-week ratio from week 3 on."""
    prof, tail = {}, {}
    for m in MARKETS:
        by_w = {w: [] for w in range(weeks + 3)}
        for x in shows.values():
            fin = finale_day(x)
            if fin is None or x["aired"][0] > 0:
                continue
            idx = {d: i for i, d in enumerate(x["days"])}

            def wk(lo):
                v = [x["att"][m][idx[d]] for d in range(lo, lo + 7) if d in idx and x["obs"][m][idx[d]]]
                return sum(v) / len(v) if len(v) >= 4 else None
            base = wk(fin)
            if not base or base <= 0:
                continue
            for w in by_w:
                v = wk(fin + 7 + 7 * w)
                if v is not None:
                    by_w[w].append(v / (base * math.exp(3.5 * drift[m])))
        prof[m] = [round(sum(v) / len(v), 6) for w, v in sorted(by_w.items()) if w < weeks and len(v) >= min_shows]
        ratios = []
        for w in range(3, weeks + 2):
            a, b = by_w.get(w, []), by_w.get(w + 1, [])
            if len(a) >= 5 and len(b) >= 5:
                ratios.append((sum(b) / len(b)) / (sum(a) / len(a)))
        tail[m] = round(min(0.95, sum(ratios) / len(ratios)), 6) if ratios else 0.78
    return prof, tail


def season_listing(inputs_dir: Path, lo=-60, hi=30):
    """How a season's shows arrive in the market: u[j] for j = lo..hi days relative to the season's start
    (1 Jan/Apr/Jul/Oct), the share of the season's pre-air attention (point-in-time us estimate at the
    premiere) from shows not yet in the inputs (not yet subscribing or listed) on that day. A season's shows
    are those premiering from 14 days before its start. Fitted on every season start the data covers with
    the whole arrival window inside the data."""
    first = {}
    days = []
    for f in sorted(inputs_dir.glob("*.json")):
        d = json.loads(f.read_text(encoding="utf-8"))
        day = date.fromisoformat(d["day"])
        days.append(day)
        for x in d["shows"]:
            if x["id"] not in first and not x["long_runner"]:
                first[x["id"]] = (day, date.fromisoformat(x["first_air"]), x)
    lo_d, hi_d = days[0], days[-1]
    weights = {}
    for sid, (seen, fa, x) in first.items():
        weights[sid] = (seen, fa)
    out = None
    used = []
    for y in range(lo_d.year, hi_d.year + 1):
        for mth in (1, 4, 7, 10):
            S = date(y, mth, 1)
            if S.toordinal() + lo < lo_d.toordinal() or S.toordinal() + hi > hi_d.toordinal():
                continue
            members = [(seen, fa, sid) for sid, (seen, fa) in weights.items() if fa.toordinal() >= S.toordinal() - 14
                       and fa.toordinal() < S.toordinal() + 77]
            # weight: the point-in-time us estimate the day before the premiere (read from the files)
            w = {}
            for seen, fa, sid in members:
                f = inputs_dir / f"{date.fromordinal(fa.toordinal() - 1).isoformat()}.json"
                if f.exists():
                    for x in json.loads(f.read_text(encoding="utf-8"))["shows"]:
                        if x["id"] == sid and x.get("prior"):
                            w[sid] = x["prior"]["us"]
            tot = sum(w.values())
            if not tot:
                continue
            u = [sum(w[sid] for seen, fa, sid in members if sid in w and seen.toordinal() > S.toordinal() + j) / tot
                 for j in range(lo, hi + 1)]
            out = u if out is None else [a + b for a, b in zip(out, u)]
            used.append(S.isoformat())
    if out is None:
        return None, used
    return [round(v / len(used), 6) for v in out], used


def joint_filter(prior, obs_by_m, prem, P):
    """The joint filter (engine/valuation.py kf_*_joint) over one show's observations of all markets, in day
    order (us, cn, jp on the same day). Returns (log-likelihood, [(market, y, predicted mean, variance)])."""
    ev = sorted((d, MARKETS.index(m), y_mean, n) for m, obs in obs_by_m.items() for d, y_mean, n in obs)
    if not ev or prior is None:
        return 0.0, []
    mean = [P["beta"][i] * prior + P["b"][i] for i in range(3)]
    cov = V.prior_cov(P["p0"], P["rho"])
    day = prem
    ll, preds = 0.0, []
    for d, i, y_mean, n in ev:
        y = V.week_observation(y_mean, n, d_at(P["drift"][i], prem, d))
        if d > day:
            cum = [d_cum(P["drift"][k], prem, day, d) / (d - day) for k in range(3)]
            mean, cov = V.kf_predict_joint(mean, cov, d - day, cum, P["q"])
        day = d
        mu, var = mean[i], cov[i][i]
        mean, cov, l, s_ = V.kf_update_joint(mean, cov, i, y, P["r"][i], CLIP)
        ll += l
        preds.append((i, y, mu, s_))
    return ll, preds


def joint_data(shows, curve_us, keep=lambda sid, prem: True):
    out = []
    for sid, x in sorted(shows.items()):
        obs_by_m, prior, prem = {}, None, None
        for m in MARKETS:
            pr, obs = observations(x, m, curve_us)
            if pr is not None:
                prior = pr
            if obs and x["src"][m]:
                obs_by_m[m] = obs
        ad = air_days(x)
        if prior is None or not obs_by_m or not ad:
            continue
        prem = ad[0]
        if keep(sid, prem):
            out.append((prior, obs_by_m, prem))
    return out


def joint_P(stage1, x):
    # the log level of market m at the premiere ~ b_m + beta_m log(us pre-air estimate): beta_us = 1; cn and jp
    # get their own elasticity (a show that is big on AniList is relatively smaller in China and Japan)
    return {"drift": [stage1[m][0] for m in MARKETS], "q": [stage1[m][1] for m in MARKETS],
            "r": [stage1[m][2] for m in MARKETS], "b": list(x[0:3]), "p0": [math.exp(v) for v in x[3:6]],
            "rho": math.tanh(x[6]), "beta": [1.0, x[7], x[8]]}


def fit_joint(data, stage1):
    x0 = [stage1[m][4] for m in MARKETS] + [math.log(stage1[m][3]) for m in MARKETS] + [math.atanh(0.5), 1.0, 1.0]
    steps = [0.3, 0.3, 0.3, 0.5, 0.5, 0.5, 0.5, 0.2, 0.2]
    best, nll = nelder_mead(lambda x: -sum(joint_filter(p, o, d, joint_P(stage1, x))[0] for p, o, d in data),
                            x0, steps, iters=2000)
    return joint_P(stage1, best), -nll


def level_unbiased_bias(data, P):
    """Per market, the prior bias that makes the prior unbiased in LEVEL (the value is a sum of levels, and
    so are the market totals every share is measured against): over the shows, the mean of the first
    observation's level equals the prior's expected level, exp(mean + S/2), S its predictive variance. The
    maximum-likelihood bias is unbiased in log; with prior variances near 1 the two differ, and a log-unbiased
    prior would make every show's first data lower the market total on average, a predictable rise of every
    other show's share."""
    out = list(P["b"])
    for i in range(3):
        acc, n, svar = 0.0, 0, 0.0
        for prior, obs_by_m, prem in data:
            obs = obs_by_m.get(MARKETS[i])
            if not obs:
                continue
            d, y_mean, k = obs[0]
            y = V.week_observation(y_mean, k, d_at(P["drift"][i], prem, d))
            dt = d - prem
            s_ = P["p0"][i] + P["q"][i] * dt + P["r"][i]
            acc += math.exp(y - P["beta"][i] * prior - d_cum(P["drift"][i], prem, prem, d) - 0.5 * s_)
            n += 1
        if n:
            out[i] = math.log(acc / n)
    return out


def joint_coverage(data, P):
    inside = {m: 0 for m in MARKETS}
    total = {m: 0 for m in MARKETS}
    for p, o, d in data:
        for i, y, mu, s_ in joint_filter(p, o, d, P)[1]:
            total[MARKETS[i]] += 1
            inside[MARKETS[i]] += abs(y - mu) <= Z80 * math.sqrt(s_)
    return {m: (inside[m] / total[m] if total[m] else None, total[m]) for m in MARKETS}


def fit_stage1(shows, curve_us, keep=lambda sid, prem: True):
    """Per-market drift, q, r by maximum likelihood of each market's own filter, started from each show's first
    observation (the dynamics only: the prior is the joint stage's business). b and p0 here are starting values
    for the joint stage (from a separate fit with the prior)."""
    out = {}
    for m in MARKETS:
        data = []
        for sid, x in sorted(shows.items()):
            prior, obs = observations(x, m, curve_us)
            ad = air_days(x)
            if obs and len(obs) >= 3 and x["src"][m] and keep(sid, ad[0]):
                data.append((prior, obs, ad[0]))
        x0 = [-0.003, math.log(0.002), math.log(0.05), math.log(0.5), 0.0, -2.0, math.log(0.5), -0.003, -0.003]
        steps = [0.002, 1.0, 1.0, 1.0, 0.3, 1.0, 1.0, 0.002, 0.002]
        dyn = [(None, o, d) for _, o, d in data]
        best, _ = nelder_mead(lambda x: -total_ll(dyn, to_prm(x)), x0, steps, iters=3000)
        d_ = to_prm(best)
        with_prior = [(p_, o, d) for p_, o, d in data if p_ is not None]
        # b and p0 starting values, the dynamics fixed
        bx, _ = nelder_mead(lambda x: -total_ll(with_prior, (d_[0], d_[1], d_[2], math.exp(x[0]), x[1], 0.0, d_[6])),
                            [math.log(0.5), 0.0], [1.0, 0.5], iters=400)
        out[m] = (d_[0], d_[1], d_[2], math.exp(bx[0]), bx[1], 0.0, d_[6])
    return out


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
    # the observation noise is floored at 1e-4: cn/jp are rebuilt series and can look noise-free in-sample;
    # the drift by period: weeks 0-3 (x[0]), 4-7 (x[7]), 8+ (x[8]) since the premiere
    pi = 0.0 if GAUSSIAN else 1 / (1 + math.exp(-x[5]))
    dr = (x[0], x[7], x[8]) if len(x) > 8 else (x[0], x[0], x[0])
    return (dr, math.exp(x[1]), max(1e-4, math.exp(x[2])), math.exp(x[3]), x[4], pi, math.exp(x[6]))


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
        lines.append("Plain Kalman filter (`--gaussian`, no outlier component): the outlier mixture was also fitted in "
                     "v0.3.0 and gave no better held-out coverage, so the simpler model is used.\n")
    BUMP.update(fit_finale_bump(shows))
    res["finale_bump"] = dict(BUMP)
    lines.append(f"Finale week: attention relative to the level two to three weeks before it: {BUMP}.\n")
    # stage 1: each market's own dynamics (drift, q, r) by maximum likelihood of its own filter
    st1 = fit_stage1(shows, curve_us)
    # stage 2: the joint prior (bias and variance per market, common correlation rho) by maximum likelihood of
    # the joint filter, over the shows that premiered inside the data (they have a point-in-time prior)
    data = joint_data(shows, curve_us)
    P, ll = fit_joint(data, st1)
    b_mle = list(P["b"])
    P["b"] = level_unbiased_bias(data, P)
    cov_in = joint_coverage(data, P)
    # held out: both stages fitted on even AniList ids, tested on odd
    even = lambda sid, prem: sid % 2 == 0
    P_even, _ = fit_joint(joint_data(shows, curve_us, even), fit_stage1(shows, curve_us, even))
    P_even["b"] = level_unbiased_bias(joint_data(shows, curve_us, even), P_even)
    cov_odd = joint_coverage(joint_data(shows, curve_us, lambda sid, prem: sid % 2 == 1), P_even)
    # out of season: fitted on the shows premiering before mid-September, tested on the later ones, and back
    split = date(2026, 9, 15).toordinal()
    summer = lambda sid, prem: prem < split
    autumn = lambda sid, prem: prem >= split
    P_s, _ = fit_joint(joint_data(shows, curve_us, summer), fit_stage1(shows, curve_us, summer))
    P_a, _ = fit_joint(joint_data(shows, curve_us, autumn), fit_stage1(shows, curve_us, autumn))
    P_s["b"] = level_unbiased_bias(joint_data(shows, curve_us, summer), P_s)
    P_a["b"] = level_unbiased_bias(joint_data(shows, curve_us, autumn), P_a)
    cov_s2a = joint_coverage(joint_data(shows, curve_us, autumn), P_s)
    cov_a2s = joint_coverage(joint_data(shows, curve_us, summer), P_a)
    n_s, n_a = len(joint_data(shows, curve_us, summer)), len(joint_data(shows, curve_us, autumn))
    for k, m in enumerate(MARKETS):
        res["drift"][m] = round(st1[m][0][0], 6)
        res.setdefault("drift_by_week", {})[m] = [round(v, 6) for v in st1[m][0]]
        res["q"][m], res["r"][m] = round(st1[m][1], 6), round(st1[m][2], 6)
        res["p0"][m], res["prior_bias"][m] = round(P["p0"][k], 6), round(P["b"][k], 6)
        res.setdefault("prior_bias_mle", {})[m] = round(b_mle[k], 6)
        res["outlier_p"][m], res["outlier_r"][m] = 0.0, round(st1[m][6], 6)
        fmt = lambda c: f"{c[0]:.1%} ({c[1]})" if c[0] is not None else "–"
        res["fit"][m] = {"coverage80_in_sample": cov_in[m][0], "observations": cov_in[m][1],
                         "coverage80_held_out": cov_odd[m][0], "held_out_observations": cov_odd[m][1],
                         "coverage80_summer_to_autumn": cov_s2a[m][0], "autumn_observations": cov_s2a[m][1],
                         "coverage80_autumn_to_summer": cov_a2s[m][0], "summer_observations": cov_a2s[m][1],
                         "prior_bias_summer_fit": round(P_s["b"][k], 6), "prior_bias_autumn_fit": round(P_a["b"][k], 6)}
        lines.append(f"## {m}\n\ndrift by weeks since the premiere (0-3, 4-7, 8+) {[round(v, 5) for v in st1[m][0]]}/day, q {st1[m][1]:.5f}/day, "
                     f"r {st1[m][2]:.4f} (own filter); prior: bias {P['b'][k]:+.3f} + {P['beta'][k]:.3f} × log(us pre-air estimate) (level-unbiased; "
                     f"maximum likelihood {b_mle[k]:+.3f}), "
                     f"variance {P['p0'][k]:.3f} (joint filter).\n\n"
                     f"80% interval coverage of the next observation: {fmt(cov_in[m])} in sample; {fmt(cov_odd[m])} on "
                     f"held-out shows (fit on even ids, tested on odd); {fmt(cov_s2a[m])} on autumn shows with the "
                     f"summer fit; {fmt(cov_a2s[m])} on summer shows with the autumn fit. Prior bias fitted on summer "
                     f"alone {P_s['b'][k]:+.3f}, on autumn alone {P_a['b'][k]:+.3f}.\n")
    growth, counts = preair_growth(shows)
    res["preair_growth"] = growth
    lines.append(f"## Pre-air estimate until the premiere\n\nMean log growth of the point-in-time estimate from k days "
                 f"before the day before the premiere (k = 7, 14, 21, 28): "
                 f"{[growth[k][0] for k in (7, 14, 21, 28)]}, variance {[growth[k][1] for k in (7, 14, 21, 28)]} "
                 f"({[counts[k] for k in (7, 14, 21, 28)]} shows).\n")
    pf, pt = post_finale(shows, {m: res["drift_by_week"][m][2] for m in MARKETS})
    res["post_finale"], res["post_finale_tail"] = pf, pt
    lines.append(f"## After the finale\n\nAttention by week after the finale week, relative to the level at its end "
                 f"(us observed, cn/jp the rebuilt series the revenue uses): {pf}; then ×{pt} a week. The game's "
                 f"`[valuation] post_finale_profile` (measured on AniList in v1) was [0.40, 0.30, 0.22, 0.16, 0.14], ×0.78.\n")
    u, seasons = season_listing(a.inputs)
    if u:
        res["season_listing"] = {"from_day": -60, "u": u, "seasons": seasons}
        lines.append(f"## Arrival of a season's shows\n\nShare of a season's pre-air attention not yet listed, by days "
                     f"from the season start (−30, −14, 0, +14, +30): {[u[k + 60] for k in (-30, -14, 0, 14, 30)]} "
                     f"(seasons: {seasons}).\n")
    res["rho"] = round(P["rho"], 6)
    res["prior_beta"] = {m: round(P["beta"][k], 6) for k, m in enumerate(MARKETS)}
    res["prior"] = "us"
    res["fit"]["joint"] = {"shows": len(data), "loglik": ll, "rho": P["rho"], "rho_summer_fit": P_s["rho"],
                           "rho_autumn_fit": P_a["rho"], "summer_shows": n_s, "autumn_shows": n_a}
    lines.insert(3, f"Joint filter (v0.3.2): one state per show, the three markets' log levels. Prior at the premiere: "
                    f"log(point-in-time us pre-air estimate) + bias_m, variances p0_m, common correlation rho = "
                    f"{P['rho']:.3f} (summer fit {P_s['rho']:.3f}, autumn fit {P_a['rho']:.3f}). {len(data)} shows with a "
                    f"prior ({n_s} premiering before 2026-09-15, {n_a} after). Two stages: each market's drift, q, r "
                    f"from its own filter; then bias, p0 and rho by maximum likelihood of the joint filter.\n")
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
