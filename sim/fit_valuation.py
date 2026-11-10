"""Fit the valuation filter (engine/valuation.py, docs/design/valuation.md §3.2) to a season of daily inputs.

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
from datetime import date, timedelta
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
                                           "long_runner": s["long_runner"], "episodes": s.get("episodes"),
                                           "prequel": False})
            x["prequel"] = x["prequel"] or bool(s.get("prequel"))
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


def finale_bump_var(shows):
    """Per market, the spread across shows (variance of the log) of the finale week's attention relative to the level
    two to three weeks before it. The bump differs by show (AniList records completions, and bigger shows record
    relatively more: in the 2026 inputs the bigger half's is 1.65, the smaller half's 1.43), so a finale-week
    observation read through the average bump carries this much extra noise about the level. Since the long tail
    multiplies whatever level the finale week suggests, reading it as exact made big shows' values jump at the
    finale and fall back when the post-finale weeks came in."""
    out = {}
    for m in MARKETS:
        v = []
        for x in shows.values():
            fin = finale_day(x)
            if fin is None or x["aired"][0] > 0 or len(air_days(x)) < 10:
                continue
            idx = {d: i for i, d in enumerate(x["days"])}

            def wk(lo):
                w = [x["att"][m][idx[d]] for d in range(lo, lo + 7) if d in idx and x["obs"][m][idx[d]]]
                return sum(w) / len(w) if len(w) >= 4 else None
            a, b, c = wk(fin), wk(fin - 14), wk(fin - 21)
            if a and b and c and b > 0 and c > 0:
                v.append(math.log(a / ((b + c) / 2)))
        mu = sum(v) / len(v) if v else 0.0
        out[m] = round(sum((y - mu) ** 2 for y in v) / len(v), 6) if len(v) >= 20 else 0.0
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
        if m != "us":
            # cn/jp: a new value of the weekly series. Compared as reported, before the finale bump: the finale's
            # week starts on its own calendar, and last week's value read again through the bump is not news
            if prev is not None and v == prev:
                continue
            prev = v
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
            out.append((d, math.log(v), 1))
    return (math.log(prior) if prior > 0 else None), out


def d_at(dr, prem, t):
    """Daily drift on day t of a show premiering on prem: dr = (weeks 0-3, 4-7, 8+) or (weeks 0, 1, 2, 3, 4-7, 8+)."""
    return dr[V.drift_seg(len(dr), (t - prem) // 7)]


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


def preair_trend(shows, growth, kmax=35, lag=7, min_shows=15):
    """A show's own pre-air growth (docs/design/valuation.md §3.2): its estimate's growth surprise over the last `lag`
    days, log(p_k / p_{k+lag}) − (g(k+lag) − g(k)), predicts its surprise until the freeze, log(p_0 / p_k) − g(k),
    almost one for one a week before (shows that gain lists fast keep gaining them: 2026, corr 0.89). For k days
    before the freeze, the least-squares slope beta_k through the origin and the variance left, [beta_k, var_k];
    the server adds beta_k × the show's surprise to the prior and uses var_k for the growth's uncertainty. Shows
    whose estimate does not reach back `lag` days use g(k) alone."""
    g = lambda k: growth[min(k, len(growth) - 1)][0] if k > 0 else 0.0
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
        for k in range(kmax + 1):
            a, b = pts.get(prem - 1 - k), pts.get(prem - 1 - k - lag)
            if a and b and a > 0 and b > 0:
                by[k].append((math.log(a / b) - (g(k + lag) - g(k)), math.log(end / a) - g(k)))
    out, last = [], [0.0, 0.0]
    for k in range(kmax + 1):
        v = by[k]
        if k == 0:
            out.append([0.0, 0.0])
            continue
        if len(v) >= min_shows:
            sxx = sum(p * p for p, _ in v)
            beta = sum(p * q for p, q in v) / sxx if sxx > 0 else 0.0
            res = [q - beta * p for p, q in v]
            last = [round(beta, 6), round(sum(r * r for r in res) / len(res), 6)]
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


def transition_stats(shows, m, drift8, weeks=3):
    """Market m's attention in weeks 0..weeks-1 after the finale's week relative to the finale week's (= level ×
    bump at the finale, moved to the week's end with the late-season drift), over the finished shows of the
    inputs: per week (t, mean of log ratios, variance of log ratios, n). Real data for us (observed daily); cn/jp
    after the finale are modelled extensions in the rebuilt series (sim/reconstruct.py) and are not used."""
    by_w = {w: [] for w in range(weeks)}
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
            if v is not None and v > 0:
                by_w[w].append(math.log(v / (base * math.exp(3.5 * drift8))))
    out = []
    for w in range(weeks):
        v = by_w[w]
        mu = sum(v) / len(v) if v else float("nan")
        out.append((7 * w + 3, mu, sum((y - mu) ** 2 for y in v) / len(v) if v else float("nan"), len(v)))
    return out


def bucket(points_by_show):
    """[(t, log mean, log variance, n)] per window position (t in days, rounded), over shows; windows with no
    activity at all are dropped (a handful)."""
    by = {}
    for pts in points_by_show:
        for t, f in pts:
            if f > 0:
                by.setdefault(round(t), []).append(math.log(f))
    out = []
    for t in sorted(by):
        v = by[t]
        if len(v) < 20:
            continue
        mu = sum(v) / len(v)
        out.append((t, mu, sum((y - mu) ** 2 for y in v) / len(v), len(v)))
    return out


def tail_var_fit(stats):
    """D(t) = a (1 - exp(-t/T)) + b t: the spread across shows of the log post-finale factor (the show-specific tail
    the filter learns as observations come: a fast part as the show settles, then a random walk), by n-weighted
    least squares on the per-window log variances. D(0) = 0: at the end of the finale's week the level is known."""
    pts = [(t, v, n) for t, _, v, n in stats]

    def f(x):
        a, tt, b = math.exp(x[0]), math.exp(x[1]), math.exp(x[2])
        return sum(n * (a * (1 - math.exp(-t / tt)) + b * t - v) ** 2 for t, v, n in pts)
    best, _ = nelder_mead(f, [math.log(0.3), math.log(30), math.log(1e-4)], [0.5, 0.5, 1.0], iters=3000)
    # T at least a day: a spread that is all there from the first day fits T -> 0, and D(t) divides by it
    return [round(math.exp(best[0]), 6), max(1.0, round(math.exp(best[1]), 4)), round(math.exp(best[2]), 9)]


def tail_curve_fit(stats, t_min):
    """The expected (mean) post-finale factor for t >= t_min: floor × yearly^(t/365) + slow excess × exp(-t/tau2) +
    fast excess × exp(-t/tau) (a floor and an excess that decays at two speeds: the months after the finale, and the
    years after), by n-weighted least squares on the log of each window's lognormal mean exp(log mean + log var/2)."""
    pts = [(t, mu + 0.5 * v, n) for t, mu, v, n in stats if t >= t_min]

    def par(x):
        tau = math.exp(x[3])
        return {"floor": math.exp(x[0]), "yearly": math.exp(-math.exp(x[1])), "excess": math.exp(x[2]), "tau": tau,
                "excess2": math.exp(x[4]), "tau2": tau + math.exp(x[5])}

    def f(x):
        p = par(x)
        return sum(n * (math.log(V.tail_formula(p, t, 1.0)) - lm) ** 2 for t, lm, n in pts)
    x0 = [math.log(0.02), math.log(0.03), math.log(0.1), math.log(100), math.log(0.05), math.log(600)]
    best, _ = nelder_mead(f, x0, [0.5] * 6, iters=6000)
    best, _ = nelder_mead(f, best, [0.2] * 6, iters=6000)
    p = par(best)
    resid = {t: lm - math.log(V.tail_formula(p, t, 1.0)) for t, lm, n in pts}
    return {k: round(v, 6) for k, v in p.items()}, resid


def solve3(A, b):
    A = [list(r) for r in A]
    b = list(b)
    for i in range(3):
        p = max(range(i, 3), key=lambda r: abs(A[r][i]))
        A[i], A[p], b[i], b[p] = A[p], A[i], b[p], b[i]
        for r in range(i + 1, 3):
            f = A[r][i] / A[i][i]
            for c in range(i, 3):
                A[r][c] -= f * A[i][c]
            b[r] -= f * b[i]
    x = [0.0] * 3
    for i in (2, 1, 0):
        x[i] = (b[i] - sum(A[i][c] * x[c] for c in range(i + 1, 3))) / A[i][i]
    return x


LIST_MIN_POP = 3000.0   # the game lists a show only with this many AniList list entries


def tail_prior_fit(us_hist, pct, ref, tail=None, drop=()):
    """Tail level by score, format and popularity (docs/design/series.md §4.1). Per show, the mean log residual of its us
    factor 3-36 months after the finale against the fitted curve's median (`tail` with no prior), regressed (OLS) on
    (score - 70)/10, ONA and the popularity fifth within its finale year (sim/long_tail_analysis.py's definition; one
    effect per fifth, the middle one the reference: the relation is U-shaped). The multiplier scales the whole
    post-transition curve (the effect is there from the first months on), centred to average 1 in level. Only public
    data at the finale (the report's other covariates include future information, e.g. a sequel airing). Held out:
    fit on even ids, R² on odd. The runtime percentile compares a show's popularity with the latest complete finale
    year's (`pop_ref`)."""
    rows = []
    for mid, m, pts in us_hist:
        if not m.get("averageScore") or mid not in pct or tail is None:
            continue
        res = [math.log(f) - (math.log(V.tail_formula(tail, t, 1.0)) - 0.5 * V.tail_var(tail, round(t)))
               for t, f in pts if 75 <= t <= 1100 and f > 0]
        if len(res) < 3:
            continue
        k = min(4, int(pct[mid] * 5))
        x = [1.0, (m["averageScore"] - 70) / 10, 1.0 if m["format"] == "ONA" else 0.0] + \
            [1.0 if (k == j and j not in drop) else 0.0 for j in (0, 1, 3, 4)]
        rows.append((mid, x, sum(res) / len(res)))

    def ols(rs):
        n = len(rs[0][1])
        A = [[sum(x[i] * x[j] for _, x, _ in rs) for j in range(n)] for i in range(n)]
        b = [sum(x[i] * y for _, x, y in rs) for i in range(n)]
        return solve_n(A, b)

    def r2(beta, rs):
        ys = [y for _, _, y in rs]
        mu = sum(ys) / len(ys)
        res = sum((y - sum(b * v for b, v in zip(beta, x))) ** 2 for _, x, y in rs)
        return 1 - res / sum((y - mu) ** 2 for y in ys), res

    def ols_(rs):
        # a dropped fifth's column is all zero: fit without it, its effect 0
        keep = [i for i in range(len(rs[0][1])) if any(x[i] for _, x, _ in rs)]
        b = ols0([(mid, [x[i] for i in keep], y) for mid, x, y in rs])
        out = [0.0] * len(rs[0][1])
        for i, v in zip(keep, b):
            out[i] = v
        return out
    ols0 = ols
    ols = ols_ if drop else ols0
    beta = ols(rows)
    r2_in, res = r2(beta, rows)
    odd = [r for r in rows if r[0] % 2 == 1]
    r2_out = r2(ols([r for r in rows if r[0] % 2 == 0]), odd)[0] if odd and len(odd) < len(rows) else float("nan")
    n = len(rows)
    sd = math.sqrt(res / (n - len(beta)))
    c = math.log(sum(math.exp(sum(b * v for b, v in zip(beta[1:], x[1:]))) for _, x, _ in rows) / n)
    years = [y for y, v in ref.items() if len(v) >= 100]
    y = max(years)
    fifth = [round(beta[3], 6), round(beta[4], 6), 0.0, round(beta[5], 6), round(beta[6], 6)]
    return {"score": round(beta[1], 6), "ona": round(beta[2], 6), "pop_fifth": fifth, "center": round(c, 6),
            "score_ref": 70.0, "pop_ref_year": y, "pop_ref": ref[y]}, n, r2_in, r2_out, sd


def solve_n(A, b):
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


def tail_coverage(us_hist, tail, months=(12, 24, 36)):
    """§11 hold-out check: share of shows whose us factor near 12/24/36 months after the finale falls in the
    model's central 80% interval for a show whose level at the finale is known: log f ~ N(log mean(t) - D(t)/2,
    D(t)) (the score prior aside)."""
    out = {}
    for mo in months:
        t0 = mo * 30.44 - 7
        inside = total = 0
        for mid, m, pts in us_hist:
            near = [(abs(t - t0), t, f) for t, f in pts if abs(t - t0) <= 10 and f > 0]
            if not near:
                continue
            _, t, f = min(near)
            d = V.tail_var(tail, t)
            med = math.log(V.tail_formula(tail, t, 1.0)) - 0.5 * d
            total += 1
            inside += abs(math.log(f) - med) <= Z80 * math.sqrt(d)
        out[mo] = (inside / total if total else None, total)
    return out


def cn_conversion(shows):
    """cn: the in-season mean of the game's cn attention (episode comments, rebuilt weekly) relative to the finale
    week's, over the finished shows of the inputs (geometric mean): the collection tail, measured against the
    season's mean (sim/calibration/long_tail_sequel.md), is put in units of the level at the finale with it."""
    v = []
    for x in shows.values():
        fin = finale_day(x)
        if fin is None or x["aired"][0] > 0 or not x["src"]["cn"]:
            continue
        idx = {d: i for i, d in enumerate(x["days"])}
        prem = air_days(x)[0]
        ins = [x["att"]["cn"][idx[d]] for d in range(prem, fin + 7) if d in idx and x["obs"]["cn"][idx[d]]]
        fw = [x["att"]["cn"][idx[d]] for d in range(fin, fin + 7) if d in idx and x["obs"]["cn"][idx[d]]]
        if len(ins) >= 28 and len(fw) >= 4 and sum(fw) > 0 and sum(ins) > 0:
            v.append(math.log((sum(ins) / len(ins)) / (sum(fw) / len(fw))))
    return math.exp(sum(v) / len(v)), len(v)


def tail_text(u):
    v = u["var"]
    return (f"transition {u['trans']}; then {u['floor']:.4f} × {u['yearly']:.4f}^(t/365) + {u['excess2']:.4f} × "
            f"exp(−t/{u['tau2']:.0f}) + {u['excess']:.4f} × exp(−t/{u['tau']:.0f}); spread D(t) = {v[0]:.3f}(1−e^(−t/"
            f"{v[1]:.0f})) + {v[2] * 365:.4f}·t/365.")


def tail_table(lines, st_, tail, t_min):
    lines.append("| t (days after finale+7) | n | median factor | log var | expected (data) | expected (model) |\n"
                 "|---|---|---|---|---|---|")
    for t, mu, v, n in st_:
        model = f"{V.tail_formula(tail, t, 1.0):.4f}" if t >= t_min else "(transition)"
        lines.append(f"| {t} | {n} | {math.exp(mu):.4f} | {v:.3f} | {math.exp(mu + 0.5 * v):.4f} | {model} |")


def tail_prior_select(hist, us_hist, tail, lines):
    """v0.6.1 (docs/design/valuation.md §3.12): the tail prior on the shows the game would list (>= LIST_MIN_POP list
    entries at the finale; below that the history only holds shows whose tail later grew), popularity fifths among
    them; a fifth's effect is kept only if the held-out half agrees (fit on even ids and on odd: same sign, the smaller
    at least half the larger), else 0. The in-season references (`pop_ref_week`) let a show not finished yet be placed
    against the reference shows at the same week since the premiere."""
    pct, ref = hist.popularity_percentiles(LIST_MIN_POP)
    full = tail_prior_fit(us_hist, pct, ref, tail)
    halves = [tail_prior_fit([h for h in us_hist if h[0] % 2 == par], pct, ref, tail)[0]["pop_fifth"] for par in (0, 1)]
    drop = tuple(j for j in (0, 1, 3, 4) if not (halves[0][j] * halves[1][j] > 0 and
                                                 min(abs(halves[0][j]), abs(halves[1][j])) >= 0.5 * max(abs(halves[0][j]), abs(halves[1][j]))))
    prior, n, r2, r2_out, sd = tail_prior_fit(us_hist, pct, ref, tail, drop) if drop else full
    prior["pop_ref_week"] = hist.popularity_by_week(prior["pop_ref_year"], LIST_MIN_POP)
    prior["min_pop"] = LIST_MIN_POP
    lines.append(f"Tail prior by popularity fifth among shows with >= {LIST_MIN_POP:.0f} list entries at the finale: all "
                 f"{full[0]['pop_fifth']}, even ids {halves[0]}, odd ids {halves[1]}; dropped (halves disagree) {list(drop)}; "
                 f"used {prior['pop_fifth']} ({n} shows).\n")
    return prior, n, r2, r2_out, sd


JP_TAIL_WINS = [(0, 7), (7, 14), (14, 21), (21, 28), (28, 56), (56, 90), (90, 150), (150, 240), (240, 400), (400, 600),
                (600, 900), (900, 1300), (1300, 1800), (1800, 2600), (2600, 3300)]


def fit_tail_jp(hist, annict_dir: Path, bump_jp: float, lines: list, today: date = date(2026, 11, 3)) -> dict:
    """jp long tail from Annict watch records, the game's jp metric (docs/design/series.md §4.2), measured like us:
    records/day in windows t days after the finale's week, relative to the mean records/day of the four weeks before
    the finale × the jp finale bump; windows from a sequel's first sign on are left out. Not deflated: the jp
    revenue divides by a fixed platform size (revenue.platform_fixed_jp), so the revenue follows the records
    themselves, and the history's records already carry Annict's growth (the in-season median per show about doubled
    2018→2025; deflated, the tail would fall ~10% a year after year 2, present value ~15% lower).

    `annict_dir`: the layout sim/backfill_annict_seasons.py + sim/backfill_annict_records.py write with
    ANISTOCK_DB=<dir>/collector.db ANISTOCK_DERIVED_DIR=<dir> (Annict works with their MAL ids, and annict_daily);
    shows join to the history by MAL id within a season of the premiere (engine/economy.py MAL_SEASON_MONTHS)."""
    import sqlite3
    from collections import defaultdict

    from engine.economy import MAL_SEASON_MONTHS, SEASON_MONTH
    from sim.tail_history import fdate
    works_db, rec_db = annict_dir / "collector.db", annict_dir / "heat_backfill.db"
    for p in (works_db, rec_db):
        if not p.exists():
            sys.exit(f"{p} missing: the jp tail needs the Annict records backfill (sim/backfill_annict_records.py)")
    al_of_mal = {}
    for mid, m in hist.media.items():
        if m.get("idMal"):
            al_of_mal.setdefault(str(m["idMal"]), mid)
    annict_of = {}
    for ext, d in sqlite3.connect(f"file:{works_db}?mode=ro", uri=True).execute(
            "SELECT external_id, data FROM snapshots WHERE source='annict' AND kind='work'"):
        w = json.loads(d)
        mid = al_of_mal.get(str(w.get("malAnimeId")))
        s = fdate(hist.media[mid]["startDate"]) if mid in hist.media else None
        if s is None or not w.get("seasonYear") or w.get("seasonName") not in SEASON_MONTH:
            continue
        if abs(s.year * 12 + s.month - (w["seasonYear"] * 12 + SEASON_MONTH[w["seasonName"]])) > MAL_SEASON_MONTHS:
            continue
        annict_of[mid] = int(ext)
    rec = defaultdict(dict)
    for aid, day, n in sqlite3.connect(f"file:{rec_db}?mode=ro", uri=True).execute(
            "SELECT annict_id, day, records FROM annict_daily"):
        rec[aid][date.fromisoformat(day)] = n

    def mean(aid, a, b):
        n = (b - a).days + 1
        return sum(rec[aid].get(a + timedelta(days=k), 0) for k in range(n)) / n

    pts = []
    for mid, m, s, e in hist.finished_shows():
        aid = annict_of.get(mid)
        if aid is None or not rec.get(aid):
            continue
        ref = mean(aid, max(s, e - timedelta(days=28)), e - timedelta(days=1))
        if ref <= 0:
            continue
        ann = hist.announced(m)
        f0 = e + timedelta(days=7)
        p = []
        for lo, hi in JP_TAIL_WINS:
            a, b = f0 + timedelta(days=lo), f0 + timedelta(days=hi - 1)
            if b > today or (ann is not None and ann <= b):
                continue
            p.append(((lo + hi) / 2, mean(aid, a, b) / ref / bump_jp))
        pts.append(p)
    st_ = bucket(pts)
    curve, _ = tail_curve_fit(st_, t_min=28)
    out = {"trans": [round(math.exp(mu + 0.5 * v), 6) for t, mu, v, n in st_ if t < 28], **curve,
           "var": tail_var_fit(st_)}
    lines.append(f"\njp: {len(pts)} finished shows with Annict watch records (TV/ONA 2018–2025 of the history, joined by "
                 f"MAL id), records/day relative to the 4 weeks before the finale × finale bump {bump_jp:.3f}; not "
                 f"deflated (the jp platform size is fixed).\n")
    tail_table(lines, st_, out, 28)
    lines.append(f"\njp: {tail_text(out)}\n")
    return out


def fit_tail(hist_path, shows, drift8, bump, lines, annict_dir: Path | None = None):
    """The long tail after the finale (docs/design/series.md §4), per market: a weekly transition, then floor ×
    yearly^(t/365) + slow excess × exp(-t/tau2) + fast excess × exp(-t/tau) (the expected factor relative to the level at the finale × the finale bump,
    t days after the finale's week), the spread D(t) of the show-specific tail, and the score/format prior. us: the
    first weeks from the inputs (observed), then the history backfill deflated by the platform index; cn: Bangumi
    collection comments in the history backfill (the collection proxy); jp: Annict watch records (`fit_tail_jp`)."""
    from sim.tail_history import History
    hist = History(str(hist_path))
    out = {}
    us_tr = [s for s in transition_stats(shows, "us", drift8["us"], weeks=3) if s[3] >= 10]
    us_hist = hist.us_tail(bump["us"])
    us_st = us_tr + bucket(pts for _, _, pts in us_hist)
    curve, _ = tail_curve_fit(us_st, t_min=21)
    out["us"] = {"trans": [round(math.exp(mu + 0.5 * v), 6) for t, mu, v, n in us_tr], **curve,
                 "var": tail_var_fit(us_st)}
    lines.append("## Long tail (docs/design/series.md §4)\n")
    lines.append(f"Expected attention t days after the finale's week = level at the finale × finale bump × factor(t): "
                 f"weekly transition, then floor × yearly^(t/365) + slow excess × exp(−t/τ2) + fast excess × exp(−t/τ), the floor and the slow excess scaled by the score "
                 f"prior. The filter reads post-finale observations through the median factor (expected × exp(−D(t)/2)), "
                 f"the level a random walk whose variance grows by D(t), the spread of show-specific tails.\n")
    lines.append(f"us: {len(us_hist)} finished shows of the history backfill (TV/ONA 2018–2025; windows before any "
                 f"sign of a sequel; deflated attention / (mean of the 4 weeks before the finale's week × finale bump "
                 f"{bump['us']:.3f})); weeks 0–2 from the inputs ({[s[3] for s in us_tr]} shows).\n")
    tail_table(lines, us_st, out["us"], 21)
    u = out["us"]
    lines.append(f"\nus: {tail_text(u)}\n")
    # the data report's simple form, relative to the in-season mean (medians): 0.025 + 0.14 e^(-t/7 months)
    ins_us = hist_in_season_ratio(hist, bump["us"])
    lines.append(f"Cross-check (data report: median ≈ 0.025 + 0.14·e^(−t/7 months) of the in-season mean; here the "
                 f"median factor × level·bump / in-season mean = × {ins_us:.3f}): " + ", ".join(
                     f"{mo} m: {0.025 + 0.14 * math.exp(-mo / 7):.3f} vs "
                     f"{V.tail_formula(u, mo * 30.44, 1.0) * math.exp(-0.5 * V.tail_var(u, mo * 30.44)) * ins_us:.3f}"
                     for mo in (1, 3, 6, 12, 24, 36, 60, 96)) + ".\n")
    conv, nconv = cn_conversion(shows)
    wins = [(0, 7), (7, 14), (14, 21), (21, 28), (28, 56), (56, 90), (90, 150), (150, 240), (240, 400), (400, 600),
            (600, 900), (900, 1300), (1300, 1800), (1800, 2600)]
    cn_hist = hist.cn_collection_vs_season(wins)
    cn_st = [(t, mu + math.log(conv / bump["cn"]), v, n) for t, mu, v, n in bucket(pts for _, pts in cn_hist)]
    curve_cn, _ = tail_curve_fit(cn_st, t_min=28)
    out["cn"] = {"trans": [round(math.exp(mu + 0.5 * v), 6) for t, mu, v, n in cn_st if t < 28], **curve_cn,
                 "var": tail_var_fit(cn_st)}
    c = out["cn"]
    lines.append(f"\ncn: {len(cn_hist)} shows with Bangumi subject (collection) comments; the collection rate relative "
                 f"to the season's mean (incl. the two weeks after the finale) × {conv:.3f} (the game's cn attention: "
                 f"in-season mean / finale week, {nconv} shows of the inputs) ÷ finale bump {bump['cn']:.3f}.\n")
    tail_table(lines, cn_st, c, 28)
    lines.append(f"\ncn: {tail_text(c)}\n")
    if annict_dir is None:
        sys.exit("--history without --annict: the jp tail is measured on the Annict records backfill (fit_tail_jp)")
    out["jp"] = fit_tail_jp(hist, annict_dir, bump["jp"], lines)
    out["jp_source"] = "annict"
    prior, n, r2, r2_out, sd = tail_prior_select(hist, us_hist, out["us"], lines)
    out["prior"] = prior
    f5 = prior["pop_fifth"]
    lines.append(f"Tail level prior (us, mean log residual 3–36 months after the finale against the curve, {n} shows): "
                 f"{prior['score']:.3f}·(score−70)/10 + {prior['ona']:.3f}·ONA + popularity fifth within the finale "
                 f"year {f5} (the middle fifth the reference) − {prior['center']:.3f} (R² {r2:.3f}, held out "
                 f"{r2_out:.3f}; residual sd {sd:.2f}). The relation with popularity is U-shaped, so one effect per "
                 f"fifth. Popularity is compared with the {prior['pop_ref_year']} finales (`pop_ref`). Scales the whole "
                 f"post-transition curve, all markets. The data report's −1.035 for a linear percentile is a different "
                 f"normalisation (relative to the in-season mean, sequels included, a 'sequel aired' covariate).\n")
    even = [h for h in us_hist if h[0] % 2 == 0]
    odd = [h for h in us_hist if h[0] % 2 == 1]
    ev_st = us_tr + bucket(pts for _, _, pts in even)
    ev = {**tail_curve_fit(ev_st, 21)[0], "var": tail_var_fit(ev_st), "trans": out["us"]["trans"]}
    cov = tail_coverage(odd, ev)
    cov_in = tail_coverage(us_hist, out["us"])
    lines.append("Hold-out (§11): share of us factors inside the 80% interval (fit on even AniList ids, tested on "
                 "odd; in sample in brackets): " + ", ".join(
                     f"{mo} months {c_[0]:.1%} of {c_[1]} ({cov_in[mo][0]:.1%})" for mo, c_ in cov.items()) + ".\n")
    return out


def hist_in_season_ratio(hist, bump):
    """Level at the finale × bump relative to the in-season mean (us, history backfill, geometric mean)."""
    v = []
    for mid, m, s, e in hist.finished_shows():
        ref = hist.window_att(mid, max(s, e - timedelta(days=28)), e - timedelta(days=1))
        ins = hist.window_att(mid, s, e)
        if ref > 0 and ins > 0:
            v.append(math.log(ref * bump / ins))
    return math.exp(sum(v) / len(v))


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
    # get their own elasticity (a show that is big on AniList is relatively smaller in China and Japan).
    # x[6] the (us-cn, or common) correlation; with x[9], x[10] the us-jp and cn-jp ones (v0.6.1 pairwise)
    rho = [math.tanh(x[6]), math.tanh(x[9]), math.tanh(x[10])] if len(x) > 10 else math.tanh(x[6])
    return {"drift": [stage1[m][0] for m in MARKETS], "q": [stage1[m][1] for m in MARKETS],
            "r": [stage1[m][2] for m in MARKETS], "b": list(x[0:3]), "p0": [math.exp(v) for v in x[3:6]],
            "rho": rho, "beta": [1.0, x[7], x[8]]}


def corr_pd(rho) -> bool:
    """The 3x3 correlation matrix of `rho` (common or pairwise) is positive definite."""
    a, b, c = rho if isinstance(rho, list) else (rho, rho, rho)
    return max(abs(a), abs(b), abs(c)) < 1.0 and 1.0 + 2.0 * a * b * c - a * a - b * b - c * c > 1e-9


def fit_joint(data, stage1, pairwise=True):
    """Bias, p0, correlations and the cn/jp elasticities by maximum likelihood of the joint filter (the dynamics
    fixed from stage 1). `pairwise`: three correlations (v0.6.1), else one common rho (v0.3.2)."""
    # (beta_us stays 1: fitting it with the dynamics fixed gave 0.95, held out 0.98, log-likelihood +0.3 — no evidence
    # that the us pre-air estimate overstates big shows)
    x0 = [stage1[m][4] for m in MARKETS] + [math.log(stage1[m][3]) for m in MARKETS] + [math.atanh(0.5), 1.0, 1.0]
    steps = [0.3, 0.3, 0.3, 0.5, 0.5, 0.5, 0.5, 0.2, 0.2]
    if pairwise:
        x0 += [math.atanh(0.5), math.atanh(0.5)]
        steps += [0.5, 0.5]

    def nll(x):
        P = joint_P(stage1, x)
        if not corr_pd(P["rho"]):
            return 1e18
        return -sum(joint_filter(p, o, d, P)[0] for p, o, d in data)
    best, v = nelder_mead(nll, x0, steps, iters=3000 if pairwise else 2000)
    return joint_P(stage1, best), -v


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


def close_days(x, cal, partial=None, drift_heat=None):
    """One show through the close's observation logic, day by day (backend/src/vclose.rs valuation_step, in season:
    from the premiere to a week after the finale; shows premiering inside the data, with a point-in-time prior). The
    joint state `base` takes the complete us episode weeks and the new cn/jp values; with `partial` (the
    `partial_us` fit) the days so far of an episode week are observed on top of it (`post`), replaced every day.
    Returns (complete weeks, days): a week is {ys, seg, heat, premiere, finale, mu, s0, drift} (its 7 daily log values
    when aligned on an air day, the base's predictive mean and variance of its observation); a day is
    (day, n, base mean, post mean, partial observation or None, base predictive variance). `drift_heat` (default the
    file's `drift_heat`): the heat class is set from the us level DRIFT_WEEKS[0] weeks after the premiere and adds its
    drift offset from then on (engine/valuation.py `heat_class`/`for_class`); each week carries "cls" and "h4"."""
    ad = air_days(x)
    if not ad or x["long_runner"] or x["aired"][0] > 0:
        return [], []
    prem = ad[0]
    idx = {d: i for i, d in enumerate(x["days"])}
    pre_i = idx.get(prem)
    if pre_i is None or x["prior_us"][pre_i] is None or x["prior_us"][pre_i] <= 0:
        return [], []
    lp = math.log(x["prior_us"][pre_i])
    dr = [tuple(cal["drift_by_week"][m]) for m in MARKETS]
    hp = {"drift_heat": drift_heat if drift_heat is not None else cal.get("drift_heat"), "drift_by_week": cal["drift_by_week"]}
    cls, h4, h4_day = None, None, None
    q = [cal["q"][m] for m in MARKETS]
    r = [cal["r"][m] for m in MARKETS]
    op = [cal["outlier_p"][m] for m in MARKETS]
    orr = [cal["outlier_r"][m] for m in MARKETS]
    bump = [cal["finale_bump"][m] for m in MARKETS]
    bvar = [cal["finale_bump_var"][m] for m in MARKETS]
    cu = cal["curve_us"]
    fp = {"partial_us": partial} if partial else {}
    bias = cal.get("prior_bias_sequel", cal["prior_bias"]) if x.get("prequel") else cal["prior_bias"]
    mean = [bias[m] + cal["prior_beta"][m] * lp for m in MARKETS]
    cov = V.prior_cov([cal["p0"][m] for m in MARKETS], V.prior_corr(cal))
    fin = finale_day(x)
    day = prem
    ys, seg, heat, prem_wk = [], None, None, False
    prev_raw = [None, None, None]
    weeks, days = [], []
    for i, d in enumerate(x["days"]):
        if d < prem or (fin is not None and d >= fin + 7):
            continue
        if d > day:
            cum = [d_cum(dr[k], prem, day, d) / (d - day) for k in range(3)]
            mean, cov = V.kf_predict_joint(mean, cov, d - day, cum, q)
        day = d
        if h4 is None and d - prem >= 7 * V.DRIFT_WEEKS[0]:
            h4, h4_day = mean[0], d
            if hp["drift_heat"]:
                cls = V.heat_class(hp, mean[0])
                dr[0] = tuple(V.for_class(hp, cls)["drift_by_week"]["us"])
        a_ = V.last_air_before(ad, d)
        k = None if a_ is None else d - a_
        in_fin = fin is not None and fin <= d < fin + 7
        drift_us = d_at(dr[0], prem, d)
        if ys and (k == 0 or len(ys) >= 7):
            covers = fin is not None and d - len(ys) <= fin + 6 and d - 1 >= fin
            xr = bvar[0] if covers else 0.0
            y = V.week_observation(sum(ys) / len(ys), len(ys), drift_us)
            weeks.append({"ys": list(ys), "seg": seg, "heat": heat, "premiere": prem_wk, "finale": covers, "cls": cls, "h4": h4, "h4_day": h4_day,
                          "mu": mean[0], "s0": cov[0][0] + r[0] + xr, "y": y, "drift": drift_us, "day": d})
            mean, cov, _, _ = V.kf_update_joint_robust(mean, cov, 0, y, r[0] + xr, op[0], orr[0], CLIP)
            ys, seg, heat, prem_wk = [], None, None, False
        att = [x["att"][m][i] for m in MARKETS]
        obs = [x["obs"][m][i] for m in MARKETS]
        raw = att
        att = [att[m] / V.runup(cal, MARKETS[m], fin, d) for m in range(3)]
        if att[0] > 0 and obs[0]:
            v = att[0] / bump[0] if in_fin else att[0]
            ys.append(math.log(v / (cu[k] if (k is not None and k < 7) else 1.0)))
        for m in (1, 2):
            if not x["src"][MARKETS[m]] or not obs[m] or att[m] <= 0:
                continue
            if prev_raw[m] is not None and raw[m] == prev_raw[m]:
                continue
            prev_raw[m] = raw[m]
            v = att[m] / bump[m] if in_fin else att[m]
            mean, cov, _, _ = V.kf_update_joint_robust(mean, cov, m, math.log(v), r[m] + (bvar[m] if in_fin else 0.0),
                                                       op[m], orr[m], CLIP)
        if k == 0 and len(ys) == 1:
            heat, prem_wk = mean[0], x["aired"][i] == 1
            # (not the finale's week: outside the fit, as in the server)
            if fin is not None and d >= fin:
                seg = None
            else:
                seg = V.partial_segment(fp, heat, prem_wk) if partial else -1
        po = None
        if partial and seg is not None and k is not None and len(ys) == k + 1:
            po = V.partial_observation(fp, seg, ys, drift_us)
        post = mean
        if po is not None:
            post, _, _, _ = V.kf_update_joint_robust(mean, cov, 0, po[0], r[0] + po[1], op[0], orr[0], CLIP)
        days.append((d, len(ys) if seg is not None and k is not None and len(ys) == k + 1 else 0, mean[0], post[0],
                     po, cov[0][0], drift_us))
    return weeks, days


def partial_segments(weeks, cuts):
    seg = lambda w: 0 if w["premiere"] else 1 + (w["heat"] >= cuts[0]) + (w["heat"] >= cuts[1])
    out = [[], [], [], []]
    for w in weeks:
        out[seg(w)].append(w)
    return out


def partial_params(weeks, min_days=2):
    """The partial-week observation from complete aligned episode weeks (not the finale's): cut points of the heat
    terciles (later weeks), and per segment the mean and covariance of d_j = ybar_j - ybar_7, j = 1..6 (maximum
    likelihood: the sample moments), turned into GLS weights on the daily values (engine/valuation.py
    `partial_observation`)."""
    ws = [w for w in weeks if len(w["ys"]) == 7 and w["heat"] is not None and not w["finale"]]
    hs = sorted(w["heat"] for w in ws if not w["premiere"])
    cuts = [hs[len(hs) // 3], hs[2 * len(hs) // 3]]
    segs = []
    for g in partial_segments(ws, cuts):
        D = [[sum(w["ys"][:j]) / j - sum(w["ys"]) / 7 for j in range(1, 7)] for w in g]
        m = [sum(r[j] for r in D) / len(D) for j in range(6)]
        S = [[sum((r[i] - m[i]) * (r[j] - m[j]) for r in D) / len(D) for j in range(6)] for i in range(6)]
        a_l, c_l, v_l = [], [], []
        for n in range(1, 7):
            gi = solve_n([row[:n] for row in S[:n]], [1.0] * n)
            v = 1.0 / sum(gi)
            gw = [z * v for z in gi]
            # weights on the daily values: ybar_j = (y_0 + ... + y_{j-1}) / j
            a = [sum(gw[j - 1] / j for j in range(kk + 1, n + 1)) for kk in range(n)]
            a_l.append([round(z, 9) for z in a])
            c_l.append(round(sum(gw[j] * m[j] for j in range(n)), 9))
            v_l.append(round(v, 9))
        segs.append({"weeks": len(g), "a": a_l, "c": c_l, "v": v_l, "mean": [round(z, 6) for z in m],
                     "var": [round(S[j][j], 6) for j in range(6)]})
    return {"min_days": min_days, "cuts": [round(c, 6) for c in cuts], "segments": segs}


def week_predictive(t, scale=1.0):
    """The predictive (mean, variance) of a complete week's observation from the base's (mu, s0) and the days so far
    (yh with extra variance v × scale): the week's observation is X, the partial one X + noise of variance v."""
    seg, n, y, mu, s0, yh, v = t
    if yh is None:
        return mu, s0
    v = v * scale
    return mu + s0 / (s0 + v) * (yh - mu), s0 * v / (s0 + v)


def week_logscore(t, scale=1.0):
    m, s = week_predictive(t, scale)
    e = t[2] - m
    return -0.5 * (math.log(2 * math.pi * s) + e * e / s)


def predictive_stats(terms, scale=None):
    """80% coverage and mean log score of the complete week's observation by n (and by segment), given `scale` per n."""
    out = {}
    for key in sorted({(t[1], None) for t in terms} | {(t[1], t[0]) for t in terms}, key=lambda z: (z[0], z[1] is not None, z[1] or 0)):
        n, seg = key
        ts = [t for t in terms if t[1] == n and (seg is None or t[0] == seg)]
        sc = (scale or {}).get(n, 1.0)
        cov = sum(abs(t[2] - week_predictive(t, sc)[0]) <= Z80 * math.sqrt(week_predictive(t, sc)[1]) for t in ts)
        out[f"n{n}" + ("" if seg is None else f"_seg{seg}")] = {
            "weeks": len(ts), "coverage80": round(cov / len(ts), 4),
            "mean_logscore": round(sum(week_logscore(t, sc) for t in ts) / len(ts), 4)}
    return out


def fit_scale(terms):
    """Per n, the factor on the partial observation's variance that maximises the likelihood of the complete weeks'
    observations given the days so far (out-of-fold terms: the covariance was fitted on other shows). Golden-section
    search on log scale in [1/4, 16]."""
    out = {}
    for n in sorted({t[1] for t in terms if t[1] > 0}):
        ts = [t for t in terms if t[1] == n]
        f = lambda ls: -sum(week_logscore(t, math.exp(ls)) for t in ts)
        lo, hi = math.log(0.25), math.log(16.0)
        g = (math.sqrt(5) - 1) / 2
        a, b = hi - g * (hi - lo), lo + g * (hi - lo)
        for _ in range(60):
            if f(a) < f(b):
                hi = b
            else:
                lo = a
            a, b = hi - g * (hi - lo), lo + g * (hi - lo)
        out[n] = round(math.exp((lo + hi) / 2), 4)
    return out


def partial_check(shows, cal, partial, keep=lambda sid: True):
    """Validation of the partial-week observation on the shows `keep` selects: (1) the predictive of each complete
    week's observation given the base state and the days so far (n = 0: the base alone), its 80% coverage and mean
    log score, by n; (2) the forecast check: the next 7 days' revision of the us level estimate (less the drift)
    against the days-so-far surprise (partial observation - base mean), with and without the partial observation."""
    terms = []
    rows = []
    for sid, x in sorted(shows.items()):
        if not keep(sid):
            continue
        weeks, days = close_days(x, cal, partial)
        for w in weeks:
            if w["seg"] is None or len(w["ys"]) != 7 or w["seg"] < 0:
                continue
            terms.append((w["seg"], 0, w["y"], w["mu"], w["s0"], None, None))
            for n in range(partial["min_days"], 7):
                po = V.partial_observation({"partial_us": partial}, w["seg"], w["ys"][:n], w["drift"])
                # (on the complete week's calendar)
                terms.append((w["seg"], n, w["y"], w["mu"], w["s0"], po[0] + (8 - n) * w["drift"], po[1]))
        by_day = {d: (bm, pm) for d, n, bm, pm, po, bv, dr in days}
        for d, n, bm, pm, po, bv, dr in days:
            if po is None or n >= 7 or (d + 7) not in by_day:
                continue
            exp_drift = 7 * dr
            s = po[0] - bm
            rows.append((s, by_day[d + 7][0] - bm - exp_drift, by_day[d + 7][1] - pm - exp_drift, n))
    out = {"predictive": predictive_stats(terms), "terms": terms}
    if rows:
        res = {}
        sx = [r_[0] for r_ in rows]
        mx = sum(sx) / len(sx)
        sxx = sum((u - mx) ** 2 for u in sx)
        for name, col in (("without", 1), ("with", 2)):
            sy = [r_[col] for r_ in rows]
            my = sum(sy) / len(sy)
            b = sum((u - mx) * (w - my) for u, w in zip(sx, sy)) / sxx
            rv = sum((w - my - b * (u - mx)) ** 2 for u, w in zip(sx, sy)) / len(sy)
            hi = [r_[col] for r_ in rows if r_[0] > math.log(1.25)]
            lo = [r_[col] for r_ in rows if r_[0] < math.log(0.875)]
            res[name] = {"after_above_25pct": math.exp(sum(hi) / len(hi)) - 1 if hi else None, "n_above": len(hi),
                         "after_below_12pct": math.exp(sum(lo) / len(lo)) - 1 if lo else None, "n_below": len(lo),
                         "slope": b, "slope_se": math.sqrt(rv / sxx)}
        out["forecast"] = res
        out["forecast_days"] = len(rows)
    return out


def fit_partial_week(shows, cal, lines=None):
    """Fit `partial_us` on all shows (the base filter's complete weeks), validate held out (fit on even AniList ids,
    tested on odd) and in sample."""
    weeks = {sid: close_days(x, cal)[0] for sid, x in shows.items()}
    pu = partial_params([w for ws in weeks.values() for w in ws])
    fold = {par: partial_params([w for sid, ws in weeks.items() if sid % 2 == par for w in ws]) for par in (0, 1)}
    oof = {par: partial_check(shows, cal, fold[par], lambda sid, par=par: sid % 2 != par) for par in (0, 1)}
    # the variance of the days so far, scaled per n by maximum likelihood out of fold (the GLS covariance fitted on
    # one half's ~120 weeks per segment understates the other half's: coverage 71-73% at n = 3..5 before)
    scale = fit_scale(oof[0]["terms"] + oof[1]["terms"])
    unscaled = predictive_stats(oof[0]["terms"])
    for p_ in [pu] + list(fold.values()):
        for sg in p_["segments"]:
            sg["v"] = [round(v * scale.get(n + 1, 1.0), 9) for n, v in enumerate(sg["v"])]
    held = partial_check(shows, cal, fold[0], lambda sid: sid % 2 == 1)
    ins = partial_check(shows, cal, pu)
    held["predictive_unscaled"] = unscaled
    for o in (held, ins):
        o.pop("terms")
    pu["variance_scale"] = scale
    # stability: the premiere segment fitted on summer and autumn premieres separately
    split = date(2026, 9, 15).toordinal()
    seasons = {}
    for name, sel in (("summer", lambda p: p < split), ("autumn", lambda p: p >= split)):
        ws = [w for x in shows.values() if air_days(x) and sel(air_days(x)[0]) for w in close_days(x, cal)[0]]
        seasons[name] = partial_params(ws)["segments"][0]["mean"]
    pu["fit"] = {"held_out": held, "in_sample": ins, "premiere_mean_by_season": seasons}
    return pu


def heat_loglik(shows, cal, dh, keep=lambda sid: True):
    """Gaussian log-likelihood of the complete us episode weeks observed after the heat class is set (the close's
    own day-by-day filter with the partial week), given the drift-by-heat table `dh` (False: none)."""
    ll, n = 0.0, 0
    for sid, x in shows.items():
        if not keep(sid):
            continue
        weeks, _ = close_days(x, cal, cal.get("partial_us"), dh)
        for w in weeks:
            if w["h4"] is None or w["day"] - 7 < w["h4_day"]:
                continue
            e = w["y"] - w["mu"]
            ll += -0.5 * (math.log(2 * math.pi * w["s0"]) + e * e / w["s0"])
            n += 1
    return ll, n


def history_heat_drift(db_path: Path):
    """The us in-season drift by heat tercile on the AniList history (sim/history_backfill.py DB, 2018-2025): us
    attention = 2 * daily list adds + trending, weekly means from the premiere; heat = mean log attention in weeks 1-3,
    ranked into terciles within the premiere's season (quarter); finished JP TV/ONA series of 10-28 episodes. Weekly log
    changes from week DRIFT_WEEKS[0] to four weeks before the finale's week (the run-in to the finale rises for
    every show and is not a heat effect). Per season, each tercile's mean minus the season's mean of the three; across
    seasons the mean and its season-clustered se; and the same on even and odd years. Returns per-day offsets."""
    import sqlite3
    from collections import defaultdict
    db = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    trend = defaultdict(dict)
    for mid, day, tr, pop in db.execute("SELECT media_id, day, trending, popularity FROM trends"):
        trend[mid][date.fromisoformat(day)] = (tr or 0, pop or 0)

    def fd(d):
        return date(d["year"], d["month"], d["day"]) if d and d.get("year") and d.get("month") and d.get("day") else None

    def week_att(mid, a):
        t = trend[mid]
        rows = [(a + timedelta(days=k), t.get(a + timedelta(days=k))) for k in range(8)]
        rows = [(d, r) for d, r in rows if r is not None]
        if len(rows) < 6:
            return None
        wk = [r for d, r in rows if d < a + timedelta(days=7)]
        tr = sum(r[0] for r in wk) / max(1, len(wk))
        adds = max(0.0, (rows[-1][1][1] - rows[0][1][1]) / max(1, (rows[-1][0] - rows[0][0]).days))
        x = 2 * adds + tr
        return x if x > 0 else None
    seasons = defaultdict(list)
    for mid, raw in db.execute("SELECT id, data FROM media WHERE in_scope=1"):
        m = json.loads(raw)
        s_, e_ = fd(m.get("startDate")), fd(m.get("endDate"))
        if not s_ or not e_ or m.get("status") != "FINISHED" or m.get("format") not in ("TV", "ONA"):
            continue
        if m.get("countryOfOrigin") != "JP" or not m.get("episodes") or not (10 <= m["episodes"] <= 28) or not (2018 <= s_.year <= 2025):
            continue
        nw = (e_ - s_).days // 7
        att = [week_att(mid, s_ + timedelta(days=7 * w)) for w in range(nw + 1)]
        if nw < 9 or any(a is None for a in att[1:4]):
            continue
        d = [math.log(att[w + 1] / att[w]) for w in range(V.DRIFT_WEEKS[0], nw - 3) if att[w] and att[w + 1]]
        if d:
            seasons[(s_.year, (s_.month - 1) // 3)].append((sum(math.log(a) for a in att[1:4]) / 3, d))
    rel = {}
    for k, xs in seasons.items():
        if len(xs) < 9:
            continue
        xs.sort(key=lambda z: z[0])
        means = []
        for t in range(3):
            v = [u for i, (_, d) in enumerate(xs) if 3 * i // len(xs) == t for u in d]
            means.append(sum(v) / len(v))
        mu = sum(means) / 3
        rel[k] = [m_ - mu for m_ in means]

    def summ(keys):
        out = []
        for t in range(3):
            v = [rel[k][t] for k in keys]
            mu = sum(v) / len(v)
            se = (sum((x - mu) ** 2 for x in v) / (len(v) - 1)) ** .5 / len(v) ** .5
            out.append((mu, se))
        return out
    allk = sorted(rel)
    res = {"seasons": len(allk), "weekly_rel": summ(allk),
           "weekly_rel_even_years": summ([k for k in allk if k[0] % 2 == 0]),
           "weekly_rel_odd_years": summ([k for k in allk if k[0] % 2 == 1])}
    res["offset"] = [round(mu / 7, 6) for mu, _ in res["weekly_rel"]]
    return res


def _excess(la):
    """Log attention of weeks -3..-1 and 0 (the finale's) relative to the level of weeks -6..-4 (centred on -5), less
    that level's own drift (weeks -7 -> -4) over the distance: {j: excess}."""
    base = (la[4] - la[7]) / 3
    ref = (la[4] + la[5] + la[6]) / 3
    return {j: la[j] - ref - base * (5 - j) for j in (3, 2, 1, 0)}


def history_runup(db_path: Path):
    """The us run-up before the finale on the AniList history (2018-2025, finished JP TV/ONA series of 10-28 episodes,
    us attention = 2 * daily list adds + trending): weekly attention aligned on the last episode's air day (week 0 =
    the finale's week, week -j the j-th before it), `_excess` per show, per season the mean; across seasons the mean,
    its season-clustered se, the median, and even / odd years."""
    import sqlite3
    from collections import defaultdict
    db = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    trend = defaultdict(dict)
    for mid, day, tr, pop in db.execute("SELECT media_id, day, trending, popularity FROM trends"):
        trend[mid][date.fromisoformat(day)] = (tr or 0, pop or 0)

    def fd(d):
        return date(d["year"], d["month"], d["day"]) if d and d.get("year") and d.get("month") and d.get("day") else None

    def week_att(mid, a):
        t = trend[mid]
        rows = [(a + timedelta(days=k), t.get(a + timedelta(days=k))) for k in range(8)]
        rows = [(d, r) for d, r in rows if r is not None]
        if len(rows) < 6:
            return None
        wk = [r for d, r in rows if d < a + timedelta(days=7)]
        tr = sum(r[0] for r in wk) / max(1, len(wk))
        adds = max(0.0, (rows[-1][1][1] - rows[0][1][1]) / max(1, (rows[-1][0] - rows[0][0]).days))
        x = 2 * adds + tr
        return x if x > 0 else None
    seasons = defaultdict(list)
    for mid, raw in db.execute("SELECT id, data FROM media WHERE in_scope=1"):
        m = json.loads(raw)
        s_, e_ = fd(m.get("startDate")), fd(m.get("endDate"))
        if not s_ or not e_ or m.get("status") != "FINISHED" or m.get("format") not in ("TV", "ONA"):
            continue
        if m.get("countryOfOrigin") != "JP" or not m.get("episodes") or not (10 <= m["episodes"] <= 28) or not (2018 <= s_.year <= 2025):
            continue
        if (e_ - s_).days < 63:
            continue
        att = {j: week_att(mid, e_ - timedelta(days=7 * j)) for j in range(8)}
        if any(v is None for v in att.values()):
            continue
        seasons[(s_.year, (s_.month - 1) // 3)].append(_excess({j: math.log(v) for j, v in att.items()}))
    per = {k: {j: sum(e[j] for e in v) / len(v) for j in (3, 2, 1, 0)} for k, v in seasons.items() if len(v) >= 8}

    def summ(keys):
        out = {}
        for j in (3, 2, 1, 0):
            z = sorted(per[k][j] for k in keys)
            mu = sum(z) / len(z)
            se = (sum((x - mu) ** 2 for x in z) / (len(z) - 1)) ** .5 / len(z) ** .5
            out[f"week-{j}"] = {"mean": round(mu, 4), "se": round(se, 4), "median": round(z[len(z) // 2], 4)}
        return out
    ks = sorted(per)
    return {"seasons": len(ks), "shows": sum(len(seasons[k]) for k in ks), "all": summ(ks),
            "even_years": summ([k for k in ks if k[0] % 2 == 0]), "odd_years": summ([k for k in ks if k[0] % 2 == 1]),
            "by_season_week-1": {f"{k[0]}Q{k[1] + 1}": round(per[k][1], 3) for k in ks}}


def calibration_runup(shows):
    """cn/jp run-up on the calibration seasons: the weekly series' value in week -j (read on its 6th day), `_excess`
    per finished show; mean and se over shows (per market)."""
    out = {}
    for m in ("cn", "jp", "us"):
        ex = []
        for x in shows.values():
            fin = finale_day(x)
            if fin is None or x["aired"][0] > 0 or not x["src"][m] or len(air_days(x)) < 10:
                continue
            idx = {d: i for i, d in enumerate(x["days"])}
            la = {}
            for j in range(8):
                if m == "us":
                    v = [x["att"][m][idx[d]] for d in range(fin - 7 * j, fin - 7 * j + 7) if d in idx and x["obs"][m][idx[d]]]
                    v = sum(v) / len(v) if len(v) >= 6 else 0
                else:
                    d = fin - 7 * j + 5
                    v = x["att"][m][idx[d]] if d in idx and x["obs"][m][idx[d]] else 0
                if v <= 0:
                    break
                la[j] = math.log(v)
            if len(la) == 8:
                ex.append(_excess(la))
        res = {}
        for j in (3, 2, 1, 0):
            z = [e[j] for e in ex]
            mu = sum(z) / len(z) if z else 0.0
            se = ((sum((v - mu) ** 2 for v in z) / (len(z) - 1)) ** .5 / len(z) ** .5) if len(z) > 2 else float("nan")
            res[f"week-{j}"] = {"mean": round(mu, 4), "se": round(se, 4)}
        out[m] = {"shows": len(ex), **res}
    return out


def fit_runup(shows, history):
    """v0.6.1 (docs/design/valuation.md §3.11): attention in the weeks before the finale's week relative to the level. us
    from the history (the season median of week -j, which a single bad season cannot move; only the weeks whose
    excess the history separates from zero), cn/jp from the calibration seasons where they separate from zero."""
    hh = history_runup(history)
    cc = calibration_runup(shows)
    res = {"us": [], "cn": [], "jp": []}
    for j in (1, 2, 3):
        a = hh["all"][f"week-{j}"]
        res["us"].append(round(math.exp(a["median"]), 6) if a["median"] > 2 * a["se"] or (j == 1 and hh["odd_years"][f"week-{j}"]["mean"] > 2 * hh["odd_years"][f"week-{j}"]["se"] and hh["even_years"][f"week-{j}"]["median"] > 0) else 1.0)
        for m in ("cn", "jp"):
            # a run-up: the week before the finale's clearly above the level (cn's excess is flat over weeks -3..0,
            # about -0.15: the reference weeks sit high, not a rise before the finale)
            c, c1 = cc[m][f"week-{j}"], cc[m]["week-1"]
            res[m].append(round(math.exp(c["mean"]), 6) if c1["mean"] > 2 * c1["se"] and c["mean"] > 2 * c["se"] else 1.0)
    for m in res:
        while res[m] and res[m][-1] == 1.0:
            res[m].pop()
    return res, {"history": hh, "calibration": cc}


def rebase_finale_bump(cal, runup):
    """The finale bump is fitted against weeks -2 and -3 (`fit_finale_bump`); with the run-up read out of those weeks
    the level is lower by their mean run-up, so the bump relative to the level is that much larger."""
    for m in MARKETS:
        r = runup.get(m) or []
        r2, r3 = (r[1] if len(r) > 1 else 1.0), (r[2] if len(r) > 2 else 1.0)
        cal["finale_bump"][m] = round(cal["finale_bump"][m] * (r2 + r3) / 2, 6)


def fit_drift_heat(shows, cal, history=None):
    """v0.6.1 (docs/design/valuation.md §3.10): the us in-season drift by heat class. A show's class is the tercile of
    its us log level estimate DRIFT_WEEKS[0] weeks after the premiere (known then; fixed after); from then on its us
    drift is the common one plus its class's offset. Cuts: the terciles over the shows; offsets: maximum likelihood
    of the complete us weeks observed after that point, the rest fixed. Validated on held-out shows (fit even ids,
    score odd, and back) and across the two seasons."""
    h4 = {}
    for sid, x in shows.items():
        weeks, _ = close_days(x, cal, cal.get("partial_us"), False)
        hs = [w["h4"] for w in weeks if w["h4"] is not None]
        if hs:
            h4[sid] = hs[0]
    hs = sorted(h4.values())
    cuts = [round(hs[len(hs) // 3], 6), round(hs[2 * len(hs) // 3], 6)]

    def fit(keep):
        f = lambda o: -heat_loglik(shows, cal, {"cuts": cuts, "offset": list(o)}, keep)[0]
        best, _ = nelder_mead(f, [0.0, 0.0, 0.0], [0.002, 0.002, 0.002], iters=200)
        return [round(v, 6) for v in best]
    off = fit(lambda sid: True)
    ll0, n = heat_loglik(shows, cal, False)
    ll1, _ = heat_loglik(shows, cal, {"cuts": cuts, "offset": off})
    fitinfo = {"calibration_ml_offset": off, "loglik_in_sample": [round(ll0, 3), round(ll1, 3)], "weeks": n}
    # the offsets used: from the history (32 seasons) when given; the two calibration seasons alone cannot tell
    # them apart from zero (held-out log-likelihood below)
    res = {"cuts": cuts, "offset": off, "shows": len(h4)}
    if history:
        hh = history_heat_drift(history)
        res["offset"] = hh["offset"]
        fitinfo["history"] = hh
        for lab, k in (("all", lambda sid: True), ("even", lambda sid: sid % 2 == 0), ("odd", lambda sid: sid % 2 == 1)):
            fitinfo[f"history_offset_loglik_{lab}"] = [round(heat_loglik(shows, cal, False, k)[0], 3),
                                                      round(heat_loglik(shows, cal, res, k)[0], 3)]
    for tr, te in ((0, 1), (1, 0)):
        o = fit(lambda sid, tr=tr: sid % 2 == tr)
        a = heat_loglik(shows, cal, False, lambda sid, te=te: sid % 2 == te)[0]
        b = heat_loglik(shows, cal, {"cuts": cuts, "offset": o}, lambda sid, te=te: sid % 2 == te)[0]
        fitinfo[f"held_out_fit{tr}_test{te}"] = {"offset": o, "loglik": [round(a, 3), round(b, 3)]}
    split = date(2026, 9, 15).toordinal()
    prem = {sid: air_days(x)[0] for sid, x in shows.items() if air_days(x)}
    for lab, tr, te in (("summer_to_autumn", lambda p_: p_ < split, lambda p_: p_ >= split),
                        ("autumn_to_summer", lambda p_: p_ >= split, lambda p_: p_ < split)):
        o = fit(lambda sid, tr=tr: sid in prem and tr(prem[sid]))
        kt = lambda sid, te=te: sid in prem and te(prem[sid])
        a = heat_loglik(shows, cal, False, kt)[0]
        b = heat_loglik(shows, cal, {"cuts": cuts, "offset": o}, kt)[0]
        fitinfo[lab] = {"offset": o, "loglik": [round(a, 3), round(b, 3)]}
    res["fit"] = fitinfo
    return res


def fit_drift_weekly(shows, cal, m):
    """v0.6.9: market m's drift by period with each of the first four weeks on its own (weeks 0, 1, 2, 3, 4-7, 8+);
    weeks 1, 2, 3 by maximum likelihood of the market's own filter (q, r, the outlier component and the other
    periods fixed at `cal`). One rate for
    weeks 0-3 projected the same fade into all four, while shows fade fastest right after the premiere: the value
    then fell predictably in week 1 and rose in week 3 (docs/design/valuation.md §3.12). Held out: fitted on even ids,
    scored on odd and back; fitted on summer, scored on autumn and back."""
    q, r, pi, r_out = cal["q"][m], cal["r"][m], cal["outlier_p"][m], cal["outlier_r"][m]
    old = tuple(cal["drift_by_week"][m])
    data = []
    for sid, x in sorted(shows.items()):
        prior, obs = observations(x, m, cal["curve_us"])
        ad = air_days(x)
        if obs and len(obs) >= 3 and x["src"][m]:
            data.append((sid, ad[0], (None, obs, ad[0])))
    prm = lambda dr: (tuple(dr), q, r, 1.0, 0.0, pi, r_out)
    ll = lambda dr, keep: total_ll([d for sid, p_, d in data if keep(sid, p_)], prm(dr))
    start = [old[0]] * 4 + list(old[-2:]) if len(old) == 3 else list(old)
    # only weeks 1-3 are free: week 0 is confounded with the prior (the first us week is observed at its end, so
    # this filter hardly sees its drift; the joint prior's bias takes it), and weeks 4-7 and 8+ keep the fit
    # with all their data (an autumn-only fit has no week 8 yet)
    expand = lambda f: [start[0], f[0], f[1], f[2], start[4], start[5]]

    def fit(keep):
        best, _ = nelder_mead(lambda f: -ll(expand(f), keep), start[1:4], [0.003] * 3, iters=1500)
        return [round(v, 6) for v in expand(best)]
    allk = lambda sid, p_: True
    new = fit(allk)
    info = {"drift_before": list(old), "loglik": [round(ll(old, allk), 3), round(ll(new, allk), 3)],
            "shows": len(data)}
    split = date(2026, 9, 15).toordinal()
    for lab, tr, te in (("even_to_odd", lambda sid, p_: sid % 2 == 0, lambda sid, p_: sid % 2 == 1),
                        ("odd_to_even", lambda sid, p_: sid % 2 == 1, lambda sid, p_: sid % 2 == 0),
                        ("summer_to_autumn", lambda sid, p_: p_ < split, lambda sid, p_: p_ >= split),
                        ("autumn_to_summer", lambda sid, p_: p_ >= split, lambda sid, p_: p_ < split)):
        d_ = fit(tr)
        info[lab] = {"drift": d_, "held_out_loglik": [round(ll(old, te), 3), round(ll(d_, te), 3)]}
    return new, info


def fit_outlier(shows, cal, m):
    """Market m's outlier component by maximum likelihood of its own filter (a profile fit: drift, q and r fixed at
    the calibration `cal`). The weekly cn/jp series and the us week average have occasional one-episode spikes
    and data glitches that a Gaussian filter reads as level changes; the mixture reads them mostly as noise."""
    global GAUSSIAN
    GAUSSIAN = False
    dyn = []
    for sid, x in sorted(shows.items()):
        prior, obs = observations(x, m, cal["curve_us"])
        if obs and len(obs) >= 3 and x["src"][m]:
            dyn.append((None, obs, air_days(x)[0]))
    dr, q, r = tuple(cal["drift_by_week"][m]), cal["q"][m], cal["r"][m]
    prm = lambda pi, ro: (dr, q, r, 0.5, 0.0, pi, ro)
    ll0 = total_ll(dyn, prm(0.0, 1.0))
    best, val = nelder_mead(lambda x: -total_ll(dyn, prm(1 / (1 + math.exp(-x[0])), math.exp(x[1]))),
                            [-3.0, math.log(0.5)], [1.0, 1.0], iters=400)
    return {"market": m, "shows": len(dyn), "observations": sum(len(o) for _, o, _ in dyn), "loglik_gaussian": ll0,
            "loglik_mixture": -val, "outlier_p": 1 / (1 + math.exp(-best[0])), "outlier_r": math.exp(best[1])}


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


def fit_flash(path: Path, r_cn: float):
    """Hourly curve and noise from comment timestamps: {episode_id: {"air": unix, "times": [unix...],
    "prev_final": [counts]}}.

    The flash reads the hour-h count as an early estimate of the episode's own final count, which is what the
    close later observes as the cn level (with noise r_cn). So its noise is r_cn plus the variance of
    log(count_h / (curve_h × final)): how far the early count lands from the episode's own final. (Until
    2026-11-07 this was the variance of log(count_h / (curve_h × previous episodes' mean)), which also counts the
    episode's real departure from the previous ones, the signal itself, as noise: the flash moved the value by a
    fraction of what it knew and the next close predictably finished the move, docs/design/api-access.md §7.)"""
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
            times = [t - e["air"] for t in e["times"] if t >= e["air"]]
            final = sum(1 for t in times if t <= 28 * 86400)
            cnt = sum(1 for t in times if t <= (h + 1) * 3600)
            if final < 20 or curve[h] <= 0 or cnt <= 0:
                continue
            z.append(math.log(cnt / (curve[h] * final)))
        noise.append(round(r_cn + st.pvariance(z), 4) if len(z) > 5 else 1.0)
    return curve, noise, len([1 for f in frac[0]])


def stage1_from(cal):
    """Stage 1 (each market's dynamics) as fitted into a parameter file: (drift by week, q, r, p0, bias, 0, r_out)."""
    return {m: (tuple(cal["drift_by_week"][m]), cal["q"][m], cal["r"][m], cal["p0"][m], cal["prior_bias_mle"][m]
                if "prior_bias_mle" in cal else cal["prior_bias"][m], 0.0, cal["outlier_r"][m]) for m in MARKETS}


def fit_sequel_bias(shows, cal) -> dict:
    """v0.8.4 (docs/design/series.md §5.3): the prior bias of shows that are themselves sequels apart from the others.
    A sequel's pre-air list entries include the earlier seasons' audience, so the same estimate means less attention
    (history backfill 2018-2026: 0.51x in weeks 2-6, 1,388 shows). Both biases are the level-unbiased bias of
    `level_unbiased_bias` over each group, every other prior parameter fixed from the file. Held out: each half (even,
    odd ids) scored with the other half's biases, one bias for all against one per group. Writes `prior_bias` (the
    shows that are not sequels) and `prior_bias_sequel`. Needs inputs with the relations (`prequel`)."""
    seqs = {sid for sid, x in shows.items() if x["prequel"]}
    if not seqs:
        sys.exit("--sequel-bias-only: no show in the inputs is a sequel (inputs without relations: sim/sequel_inputs.py)")
    st1 = stage1_from(cal)
    P = {"drift": [st1[m][0] for m in MARKETS], "q": [cal["q"][m] for m in MARKETS], "r": [cal["r"][m] for m in MARKETS],
         "b": [cal["prior_bias"][m] for m in MARKETS], "p0": [cal["p0"][m] for m in MARKETS],
         "rho": cal["prior_corr"], "beta": [cal["prior_beta"][m] for m in MARKETS]}
    cu = cal["curve_us"]

    def bias(keep):
        return level_unbiased_bias(joint_data(shows, cu, keep), P)
    first, seq = bias(lambda s_, p_: s_ not in seqs), bias(lambda s_, p_: s_ in seqs)
    ho = {}
    for tr in (0, 1):
        one = bias(lambda s_, p_: s_ % 2 == tr)
        own = {g: bias(lambda s_, p_, g=g: s_ % 2 == tr and (s_ in seqs) == g) for g in (False, True)}
        for g in (False, True):
            t = bias(lambda s_, p_, g=g: s_ % 2 != tr and (s_ in seqs) == g)
            ho[f"fit{tr}_{'sequel' if g else 'first'}"] = {
                "one_bias": [round(a - b, 3) for a, b in zip(t, one)], "by_group": [round(a - b, 3) for a, b in zip(t, own[g])]}
    n = {g: len(joint_data(shows, cu, lambda s_, p_, g=g: (s_ in seqs) == g)) for g in (False, True)}
    info = {"before": {m: cal["prior_bias"][m] for m in MARKETS}, "shows_first": n[False], "shows_sequel": n[True],
            "sequel_vs_first": {m: round(math.exp(seq[k] - first[k]), 3) for k, m in enumerate(MARKETS)},
            "held_out_level_bias": ho,
            "note": "level bias of the held-out half (log, 0 = unbiased), [us, cn, jp]; the history backfill's us "
                    "ratio for comparison: 0.509 (se 0.033)"}
    for k, m in enumerate(MARKETS):
        cal["prior_bias"][m] = round(first[k], 6)
        cal.setdefault("prior_bias_sequel", {})[m] = round(seq[k], 6)
    return info


def joint_loglik(data, P):
    return sum(joint_filter(p, o, d, P)[0] for p, o, d in data)


def fit_prior_only(shows, cal):
    """v0.6.1: the joint prior with three pairwise correlations instead of one common rho, the dynamics fixed.
    Compared with the common-rho fit in sample, on held-out shows (fit on even ids, scored on odd, and back) and
    on the cross-season split; the chosen prior's keys are written into `cal`."""
    cu = cal["curve_us"]
    st1 = stage1_from(cal)
    data = joint_data(shows, cu)
    out = {}
    for name, pw in (("common", False), ("pairwise", True)):
        P, ll = fit_joint(data, st1, pw)
        P["b_mle"] = list(P["b"])
        P["b"] = level_unbiased_bias(data, P)
        ho = {}
        for tr, te in ((0, 1), (1, 0)):
            keep_tr = lambda sid, prem, tr=tr: sid % 2 == tr
            keep_te = lambda sid, prem, te=te: sid % 2 == te
            Pt, _ = fit_joint(joint_data(shows, cu, keep_tr), st1, pw)
            Pt["b"] = level_unbiased_bias(joint_data(shows, cu, keep_tr), Pt)
            dte = joint_data(shows, cu, keep_te)
            ho[f"fit{tr}_test{te}"] = {"loglik": round(joint_loglik(dte, Pt), 3), "shows": len(dte),
                                       "coverage80": {m: (round(c[0], 4) if c[0] is not None else None, c[1])
                                                      for m, c in joint_coverage(dte, Pt).items()},
                                       "rho": Pt["rho"]}
        split = date(2026, 9, 15).toordinal()
        cs = {}
        for lab, tr, te in (("summer_to_autumn", lambda s_, p_: p_ < split, lambda s_, p_: p_ >= split),
                            ("autumn_to_summer", lambda s_, p_: p_ >= split, lambda s_, p_: p_ < split)):
            Pt, _ = fit_joint(joint_data(shows, cu, tr), st1, pw)
            Pt["b"] = level_unbiased_bias(joint_data(shows, cu, tr), Pt)
            dte = joint_data(shows, cu, te)
            cs[lab] = {"loglik": round(joint_loglik(dte, Pt), 3), "shows": len(dte), "rho": Pt["rho"],
                       "coverage80": {m: (round(c[0], 4) if c[0] is not None else None, c[1])
                                      for m, c in joint_coverage(dte, Pt).items()}}
        out[name] = {"P": P, "loglik": ll, "loglik_unbiased_bias": joint_loglik(data, P), "held_out": ho,
                     "cross_season": cs,
                     "coverage80_in_sample": {m: c[0] for m, c in joint_coverage(data, P).items()}}
        print(name, json.dumps({k: v for k, v in out[name].items() if k != "P"}, default=str))
        print("  rho", P["rho"], "p0", P["p0"], "b", P["b"], "beta", P["beta"])
    P = out["pairwise"]["P"]
    for k, m in enumerate(MARKETS):
        cal["p0"][m], cal["prior_bias"][m] = round(P["p0"][k], 6), round(P["b"][k], 6)
        cal.setdefault("prior_bias_mle", {})[m] = round(P["b_mle"][k], 6)
        cal["prior_beta"][m] = round(P["beta"][k], 6)
    cal["prior_corr"] = [round(v, 6) for v in P["rho"]]
    cal["rho"] = round(out["common"]["P"]["rho"], 6)
    cal["fit"]["joint_prior_v061"] = {
        name: {"loglik": round(o["loglik"], 3), "rho": o["P"]["rho"], "held_out": o["held_out"],
               "cross_season": o["cross_season"], "coverage80_in_sample": o["coverage80_in_sample"]}
        for name, o in out.items()}
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("inputs", type=Path)
    ap.add_argument("--out", type=Path, default=ROOT / "sim" / "calibration" / "valuation_filter.json")
    ap.add_argument("--report", type=Path)
    ap.add_argument("--comments", type=Path)
    ap.add_argument("--gaussian", action="store_true", help="no outlier component (a plain Kalman filter)")
    ap.add_argument("--clip", type=float, default=4.0)
    ap.add_argument("--history", type=Path, help="history backfill DB (sim/history_backfill.py): fit the long tail")
    ap.add_argument("--outlier-only", action="store_true",
                    help="fit only each market's outlier component (outlier_p, outlier_r) with the rest of --out fixed, "
                         "and write those two keys back")
    ap.add_argument("--preair-only", action="store_true",
                    help="refit only the pre-air estimate's growth (`preair_growth`) and the shows' own trend in it "
                         "(`preair_trend`) with the rest of --out fixed, and write those keys back")
    ap.add_argument("--partial-only", action="store_true",
                    help="fit only the us partial-week observation (`partial_us`) with the rest of --out fixed, and "
                         "write that key back")
    ap.add_argument("--annict", type=Path,
                    help="Annict records backfill directory (collector.db + heat_backfill.db, see fit_tail_jp; the "
                         "2018-2025 backfill of 2026-11-09 is data/derived/annict_hist, not in git): the jp tail")
    ap.add_argument("--tail-jp-only", action="store_true",
                    help="refit only the jp long tail (`tail.jp`; --history, --annict) with the rest of --out fixed")
    ap.add_argument("--tail-prior-only", action="store_true",
                    help="refit only the long tail's prior by popularity (`tail.prior`, --history) on the current curve")
    ap.add_argument("--runup-only", action="store_true",
                    help="fit only the run-up before the finale (`runup`; us from --history) with the rest of --out fixed, "
                         "and write that key back (then refit --heat-only and --partial-only)")
    ap.add_argument("--heat-only", action="store_true",
                    help="fit only the us drift by heat class (`drift_heat`) with the rest of --out fixed, and write that "
                         "key back (then refit --partial-only)")
    ap.add_argument("--drift-only", action="store_true",
                    help="refit only each market's drift by period, the first four weeks each on its own (`drift_by_week`, "
                         "six periods) with q, r and the outliers fixed from --out, and write that key back (then refit "
                         "--prior-only and --partial-only)")
    ap.add_argument("--prior-only", action="store_true",
                    help="refit only the joint prior (bias, p0, the pairwise correlations, cn/jp elasticities) with each "
                         "market's dynamics fixed from --out, and write those keys back (then refit --partial-only)")
    ap.add_argument("--sequel-bias-only", action="store_true",
                    help="refit only the prior bias apart for shows that are themselves sequels (`prior_bias`, "
                         "`prior_bias_sequel`), every other key fixed from --out; inputs with relations")
    a = ap.parse_args()
    global GAUSSIAN, CLIP
    GAUSSIAN, CLIP = a.gaussian, a.clip
    shows = load(a.inputs)
    if a.outlier_only:
        cal = json.loads(a.out.read_text(encoding="utf-8"))
        for m in MARKETS:
            r = fit_outlier(shows, cal, m)
            cal["outlier_p"][m], cal["outlier_r"][m] = round(r["outlier_p"], 6), round(r["outlier_r"], 6)
            print(json.dumps(r))
        a.out.write_text(json.dumps(cal, ensure_ascii=False, indent=1), encoding="utf-8")
        return
    if a.tail_jp_only:
        from sim.tail_history import History
        cal = json.loads(a.out.read_text(encoding="utf-8"))
        lines = []
        before = cal["tail"]["jp"]
        cal["tail"]["jp"] = fit_tail_jp(History(str(a.history)), a.annict, cal["finale_bump"]["jp"], lines)
        cal["tail"]["jp_source"] = "annict"
        cal.setdefault("fit", {})["tail_jp"] = {"before": before, "before_source": "the us tail", "note": lines}
        print("\n".join(lines))
        a.out.write_text(json.dumps(cal, ensure_ascii=False, indent=1), encoding="utf-8")
        return
    if a.tail_prior_only:
        from sim.tail_history import History
        cal = json.loads(a.out.read_text(encoding="utf-8"))
        hist = History(str(a.history))
        lines = []
        old = cal["tail"]["prior"]
        prior, n, r2, r2_out, sd = tail_prior_select(hist, hist.us_tail(cal["finale_bump"]["us"]), cal["tail"]["us"], lines)
        cal["tail"]["prior"] = prior
        cal.setdefault("fit", {})["tail_prior_v061"] = {"before": {k: v for k, v in old.items() if not k.startswith("pop_ref")},
                                                        "note": lines, "shows": n, "r2": r2, "r2_held_out": r2_out}
        print("\n".join(lines))
        a.out.write_text(json.dumps(cal, ensure_ascii=False, indent=1), encoding="utf-8")
        return
    if a.runup_only:
        cal = json.loads(a.out.read_text(encoding="utf-8"))
        BUMP.update(cal["finale_bump"])
        if cal.get("runup"):
            sys.exit("--runup-only: the file already has a run-up (the finale bump was rebased on it); start from one without")
        cal["runup"], info = fit_runup(shows, a.history)
        info["finale_bump_before"] = dict(cal["finale_bump"])
        rebase_finale_bump(cal, cal["runup"])
        cal.setdefault("fit", {})["runup"] = info
        print(json.dumps(cal["runup"]), json.dumps(info, indent=1))
        a.out.write_text(json.dumps(cal, ensure_ascii=False, indent=1), encoding="utf-8")
        return
    if a.heat_only:
        cal = json.loads(a.out.read_text(encoding="utf-8"))
        BUMP.update(cal["finale_bump"])
        cal["drift_heat"] = fit_drift_heat(shows, cal, a.history)
        print(json.dumps(cal["drift_heat"], indent=1))
        a.out.write_text(json.dumps(cal, ensure_ascii=False, indent=1), encoding="utf-8")
        return
    if a.drift_only:
        cal = json.loads(a.out.read_text(encoding="utf-8"))
        BUMP.update(cal["finale_bump"])
        for m in MARKETS:
            new, info = fit_drift_weekly(shows, cal, m)
            cal["drift_by_week"][m] = new
            cal["drift"][m] = new[0]
            cal.setdefault("fit", {}).setdefault("drift_weekly", {})[m] = info
            print(m, json.dumps(info))
        a.out.write_text(json.dumps(cal, ensure_ascii=False, indent=1), encoding="utf-8")
        return
    if a.sequel_bias_only:
        cal = json.loads(a.out.read_text(encoding="utf-8"))
        BUMP.update(cal["finale_bump"])
        info = fit_sequel_bias(shows, cal)
        cal.setdefault("fit", {})["prior_sequel"] = info
        print(json.dumps(info, indent=1))
        a.out.write_text(json.dumps(cal, ensure_ascii=False, indent=1), encoding="utf-8")
        return
    if a.prior_only:
        cal = json.loads(a.out.read_text(encoding="utf-8"))
        BUMP.update(cal["finale_bump"])
        fit_prior_only(shows, cal)
        a.out.write_text(json.dumps(cal, ensure_ascii=False, indent=1), encoding="utf-8")
        return
    if a.preair_only:
        cal = json.loads(a.out.read_text(encoding="utf-8"))
        growth, _ = preair_growth(shows)
        cal["preair_growth"] = growth
        cal["preair_trend"], counts = preair_trend(shows, growth)
        print(json.dumps({k: [growth[k], cal["preair_trend"][k], counts[k]] for k in (1, 3, 7, 14, 21, 28)}, indent=1))
        a.out.write_text(json.dumps(cal, ensure_ascii=False, indent=1), encoding="utf-8")
        return
    if a.partial_only:
        cal = json.loads(a.out.read_text(encoding="utf-8"))
        BUMP.update(cal["finale_bump"])
        cal["partial_us"] = fit_partial_week(shows, cal)
        print(json.dumps(cal["partial_us"]["fit"], indent=1))
        a.out.write_text(json.dumps(cal, ensure_ascii=False, indent=1), encoding="utf-8")
        return
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
    res["finale_bump_var"] = finale_bump_var(shows)
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
    trend, tcounts = preair_trend(shows, growth)
    res["preair_trend"] = trend
    lines.append(f"Own trend (beta, variance left) at k = 7, 14, 21: {[trend[k] for k in (7, 14, 21)]} "
                 f"({[tcounts[k] for k in (7, 14, 21)]} shows).\n")
    lines.append(f"## Pre-air estimate until the premiere\n\nMean log growth of the point-in-time estimate from k days "
                 f"before the day before the premiere (k = 7, 14, 21, 28): "
                 f"{[growth[k][0] for k in (7, 14, 21, 28)]}, variance {[growth[k][1] for k in (7, 14, 21, 28)]} "
                 f"({[counts[k] for k in (7, 14, 21, 28)]} shows).\n")
    pf, pt = post_finale(shows, {m: res["drift_by_week"][m][-1] for m in MARKETS})
    res["post_finale"], res["post_finale_tail"] = pf, pt
    lines.append(f"## After the finale\n\nAttention by week after the finale week, relative to the level at its end "
                 f"(us observed, cn/jp the rebuilt series the revenue uses): {pf}; then ×{pt} a week. The game's "
                 f"`[valuation] post_finale_profile` (measured on AniList in v1) was [0.40, 0.30, 0.22, 0.16, 0.14], ×0.78.\n")
    if a.history:
        res["tail"] = fit_tail(a.history, shows, {m: res["drift_by_week"][m][-1] for m in MARKETS},
                               res["finale_bump"], lines, a.annict)
    u, seasons = season_listing(a.inputs)
    if u:
        res["season_listing"] = {"from_day": -60, "u": u, "seasons": seasons}
        lines.append(f"## Arrival of a season's shows\n\nShare of a season's pre-air attention not yet listed, by days "
                     f"from the season start (−30, −14, 0, +14, +30): {[u[k + 60] for k in (-30, -14, 0, 14, 30)]} "
                     f"(seasons: {seasons}).\n")
    if GAUSSIAN:
        # the outlier component on top of the Gaussian dynamics (v0.6.0; the same as --outlier-only)
        for m in MARKETS:
            o = fit_outlier(shows, res, m)
            res["outlier_p"][m], res["outlier_r"][m] = round(o["outlier_p"], 6), round(o["outlier_r"], 6)
            lines.append(f"Outlier component, {m}: outlier_p {o['outlier_p']:.4f}, outlier_r {o['outlier_r']:.4f} (log-likelihood "
                         f"{o['loglik_gaussian']:.1f} Gaussian, {o['loglik_mixture']:.1f} mixture).\n")
        GAUSSIAN = True
    # fit_joint fits the three pairwise correlations (v0.6.1): written as prior_corr, as --prior-only does
    res["prior_corr"] = [round(v, 6) for v in P["rho"]]
    res["rho"] = round(sum(P["rho"]) / 3, 6)
    res["prior_beta"] = {m: round(P["beta"][k], 6) for k, m in enumerate(MARKETS)}
    res["prior"] = "us"
    # the us episode week's days so far (v0.6.1), on the filter above
    res["partial_us"] = fit_partial_week(shows, res)
    res["fit"]["joint"] = {"shows": len(data), "loglik": ll, "rho": P["rho"], "rho_summer_fit": P_s["rho"],
                           "rho_autumn_fit": P_a["rho"], "summer_shows": n_s, "autumn_shows": n_a}
    lines.insert(3, f"Joint filter (v0.3.2): one state per show, the three markets' log levels. Prior at the premiere: "
                    f"log(point-in-time us pre-air estimate) + bias_m, variances p0_m, common correlation rho = "
                    f"{[round(v, 3) for v in P['rho']]} (summer fit {[round(v, 3) for v in P_s['rho']]}, autumn fit {[round(v, 3) for v in P_a['rho']]}). {len(data)} shows with a "
                    f"prior ({n_s} premiering before 2026-09-15, {n_a} after). Two stages: each market's drift, q, r "
                    f"from its own filter; then bias, p0 and rho by maximum likelihood of the joint filter.\n")
    if a.comments:
        curve, noise, n = fit_flash(a.comments, res["r"]["cn"])
        res["flash_curve"], res["flash_r"] = curve, noise
        lines.append(f"## 热度快讯 hourly curve\n\n{n} episodes. Share of 28-day comments by hour 1..24: {curve}\n\n"
                     f"Noise variance of the hour-h count read as the cn level, r_cn + var log(count_h / (curve_h × own final)), "
                     f"by hour: {noise}\n")
    # the IPO discount for uncertainty (vclose.rs ipo_price_v2: 1 - exp(-ipo_risk sqrt(mean p0))). Measured on the
    # v0.3.2 replay (seed 20261105, inputs 2026-07-01..11-03): over the 61 shows listed in Aug-Oct, the value
    # 28 days after listing plus dividends, relative to the model's value when the offer was priced, is 0.935
    # value-weighted (1.158 equal-weighted, 1.024 without the top 3 hits): the model's own value already prices
    # the uncertainty, and no further discount is justified (it would only hand subscribers a riskless
    # gain; ipo_all beat cap-weighted buy-and-hold by +9.7 points with 0.25)
    res["ipo_risk"] = 0.0
    res["clip"] = CLIP
    a.out.parent.mkdir(parents=True, exist_ok=True)
    a.out.write_text(json.dumps(res, ensure_ascii=False, indent=1), encoding="utf-8")
    if a.report:
        a.report.write_text("\n".join(lines), encoding="utf-8")
    print("\n".join(lines))


if __name__ == "__main__":
    main()
