"""The pure formulas that turn the model's daily inputs (engine/daily_inputs.py) plus game state into
revenue shares and per-share values. No I/O, no Show objects: plain numbers and dicts.

This is the part the Rust game server ports (docs/v2-architecture.md §1). The simulation (sim/) uses
the same functions through engine/economy.py, so there is one copy of each formula. Every float
operation is written in a fixed order; a port must keep that order (and the summation order of its
callers) to reproduce the Python results bit for bit.
"""

from __future__ import annotations

import json
import math
from pathlib import Path

MARKETS = ("us", "cn", "jp")


def blended_share(own: dict[str, float], totals: dict[str, float], weights: dict) -> tuple[float, dict]:
    """Σ_m w_m * own_m/T_m, renormalised over markets that have any data. Returns (share, per-market part)."""
    live = [m for m in MARKETS if totals[m] > 0]
    wsum = sum(weights[m] for m in live) or 1.0
    parts = {m: (weights[m] / wsum) * own[m] / totals[m] if m in live else 0.0 for m in MARKETS}
    return sum(parts.values()), parts


def expected_share(own_id: int, listed_ids: list[int], expected: dict[int, dict[str, float]], weights: dict) -> float:
    """A show's expected share of external demand, with every listed show's expectation in the denominator
    (summed in `listed_ids` order); a show that is not listed yet is added to the denominator last."""
    tot = {m: 0.0 for m in MARKETS}
    for sid in listed_ids:
        a = expected[sid]
        for m in MARKETS:
            tot[m] += a[m]
    if own_id not in listed_ids:
        for m in MARKETS:
            tot[m] += expected[own_id][m]
    share, _ = blended_share(expected[own_id], tot, weights)
    return share


def demand_index(tot_us: float, a_ref: float, rv: dict) -> float:
    """External demand relative to the base: (AniList attention of listed shows / a_ref) ** elasticity."""
    return (tot_us / a_ref) ** rv["demand_elasticity"] if tot_us > 0 else 0


def demand_day(tot_us: float, a_ref: float, rv: dict) -> float:
    return rv["demand_base_week"] / 7 * demand_index(tot_us, a_ref, rv)


def demand_week(tot_us: float, a_ref: float, rv: dict) -> float:
    return rv["demand_base_week"] * demand_index(tot_us, a_ref, rv)


def margin(p: dict) -> float:
    """Share of revenue that reaches shareholders as dividends."""
    co = p["company"]
    return (1 - co.get("operating_cost_ratio", 0)) * (1 - co["corp_tax"]) * co["payout_ratio"]


def has_real_data(days_since_premiere: int, val: dict) -> bool:
    """Premiered long enough ago that every market has reported (cn/jp lag the premiere by a day)."""
    return days_since_premiere >= val["min_days_after_premiere"]


def fundamental(aired: int, episodes: int | None, episodes_known: bool, long_runner: bool,
                weekly_div: float, book: float, rate: float, p: dict,
                preair_div: float, use_preair: bool = False) -> float:
    """Per-share value: discounted expected dividends + book value per share.

    Expected weekly dividend runs at the current level, falls once by `premiere_drop` if only the
    premiere has aired, then by `in_season_retention` per week until the finale, then follows the
    measured post-finale profile. `episodes` is the model's count (a default when unknown);
    it may be None only for long-runners."""
    v = p["valuation"]
    rw = (rate + v["equity_premium"]) / 52
    disc = 1 / (1 + rw)
    div = preair_div if use_preair else weekly_div
    if long_runner:
        g = v["long_runner_decay"]
        pv = div * g * disc / (1 - g * disc)
    else:
        remaining = max(0, episodes - aired)
        if not episodes_known and aired > 0:
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


def fair_value_cents(revs_cents: list[int], since: int, shares: int, book_cents: int, rate: float,
                     demand_week: float, pre_share: float, aired: int, episodes: int | None,
                     episodes_known: bool, long_runner: bool, real_data: bool, p: dict) -> int:
    """Per-share value in cents from the company's own revenue history.

    revs_cents: the company's daily revenue (cents) for up to the last 7 days up to and including D,
    newest first (company_days ORDER BY day DESC LIMIT 7). since: days since the premiere + 1.
    pre_share: expected_share(); real_data: has_real_data() for D."""
    revs = [r / 100 for r in revs_cents]
    preair_weekly = demand_week * pre_share
    if since < 7:
        hist = revs[:max(1, since)]
        weekly_rev = sum(hist) + preair_weekly / 7 * (7 - len(hist))
    else:
        weekly_rev = sum(revs) * 7 / max(1, len(revs))
    weekly_div = weekly_rev * margin(p) / shares
    preair_div = preair_weekly * margin(p) / shares
    v = fundamental(aired, episodes, episodes_known, long_runner, weekly_div, book_cents / 100 / shares, rate, p,
                    preair_div, use_preair=not real_data)
    return max(1, round(v * 100))


def finished(aired: int, episodes: int | None, long_runner: bool) -> bool:
    """The last episode has aired (delisting starts counting low-revenue weeks only then)."""
    return bool((not long_runner) and episodes and aired >= episodes)


# ================================================================ filtered valuation (v0.3.0)
# docs/v2-valuation.md. The hidden value is a rational analyst's estimate from public information only:
# each show's level per market is tracked with a Kalman filter (attention = level × within-episode curve),
# the expected attention path gives expected dividends, and the value is their present value plus the
# company's cash. The game server (backend/src/valuation.rs) implements the same formulas in the same
# order. Parameters: [valuation.filter] (fitted by sim/fit_valuation.py).

_FP_CACHE: dict = {}


def filter_params(p: dict) -> dict:
    """The fitted filter parameters (sim/fit_valuation.py → sim/calibration/valuation_filter.json) plus the
    fixed settings below."""
    path = Path(__file__).resolve().parent.parent / p["valuation"].get("filter_file", "sim/calibration/valuation_filter.json")
    if path not in _FP_CACHE:
        fp = json.loads(path.read_text(encoding="utf-8"))
        fp.setdefault("clip", 4.0)
        fp.setdefault("horizon_days", 400)
        fp.setdefault("flash_min_expected", 5.0)
        fp.setdefault("ipo_risk", 0.25)
        _FP_CACHE[path] = fp
    return _FP_CACHE[path]


def curve(fp: dict, m: str, k: int | None) -> float:
    """Within-episode curve: expected attention on day k after an episode airs, relative to the weekly
    average (mean 1 over the 7 days). The us market (AniList) peaks the day after; cn/jp are rebuilt as
    weekly series, flat within the week. None: not airing (pre-premiere, break): 1."""
    if m != "us" or k is None or k >= 7:
        return 1.0
    return fp["curve_us"][k]


def kf_predict(mean: float, var: float, days: int, drift: float, q: float) -> tuple[float, float]:
    return mean + drift * days, var + q * days


def robust_update(mean: float, var: float, y: float, r: float, pi: float, r_out: float, clip: float) -> tuple[float, float, float]:
    """Kalman update with outlier-prone observations: with probability `pi` an observation carries extra
    noise `r_out` (an episode-specific spike or dip, a data glitch). The update uses the noise variance
    expected given the innovation, r + P(outlier | y) * r_out (a moment-matched Gaussian-sum update).
    Returns (mean, var, log-likelihood of y)."""
    v = y - mean
    s1 = var + r
    s2 = s1 + r_out
    l1 = (1.0 - pi) * math.exp(-0.5 * v * v / s1) / math.sqrt(2.0 * math.pi * s1)
    l2 = pi * math.exp(-0.5 * v * v / s2) / math.sqrt(2.0 * math.pi * s2)
    tot = l1 + l2
    w_out = l2 / tot if tot > 0 else 1.0
    r_eff = r + w_out * r_out
    m2, v2, _ = kf_update(mean, var, y, r_eff, clip)
    return m2, v2, math.log(tot) if tot > 0 else -1e9


def kf_update(mean: float, var: float, y: float, r: float, clip: float) -> tuple[float, float, float]:
    """One observation y (log attention / curve) with noise variance r. The innovation is clipped at
    `clip` standard deviations (robust to the data's occasional spikes). Returns (mean, var, innovation)."""
    s = var + r
    sd = s ** 0.5
    v = y - mean
    if v > clip * sd:
        v = clip * sd
    elif v < -clip * sd:
        v = -clip * sd
    k = var / s
    return mean + k * v, (1.0 - k) * var, v


# ---- the joint filter (v0.3.2): one state per show, the log levels of the three markets


def prior_cov(p0: list[float], rho: float) -> list[list[float]]:
    """Prior covariance of the three markets' log levels: variances p0, one common correlation rho (a show
    that beats its pre-air estimate in one market tends to beat it in the others)."""
    sd = [v ** 0.5 for v in p0]
    return [[p0[i] if i == j else rho * sd[i] * sd[j] for j in range(3)] for i in range(3)]


def kf_predict_joint(mean: list[float], cov: list[list[float]], days: int, drift: list[float],
                     q: list[float]) -> tuple[list[float], list[list[float]]]:
    """Each market's level drifts on its own (independent process noise q per market)."""
    m2 = [mean[i] + drift[i] * days for i in range(3)]
    c2 = [[cov[i][j] + (q[i] * days if i == j else 0.0) for j in range(3)] for i in range(3)]
    return m2, c2


def kf_update_joint(mean: list[float], cov: list[list[float]], m: int, y: float, r: float,
                    clip: float) -> tuple[list[float], list[list[float]], float, float]:
    """One observation y of market m's log level with noise variance r. Moves every market through the
    covariance. The innovation is clipped at `clip` standard deviations, as in `kf_update`. Returns (mean,
    cov, log-likelihood of y, innovation variance)."""
    s = cov[m][m] + r
    sd = s ** 0.5
    v = y - mean[m]
    ll = -0.5 * (math.log(2.0 * math.pi * s) + v * v / s)
    if v > clip * sd:
        v = clip * sd
    elif v < -clip * sd:
        v = -clip * sd
    k = [cov[i][m] / s for i in range(3)]
    m2 = [mean[i] + k[i] * v for i in range(3)]
    c2 = [[cov[i][j] - k[i] * cov[m][j] for j in range(3)] for i in range(3)]
    return m2, c2, ll, s


DRIFT_WEEKS = (4, 8)   # the in-season fade changes speed after these weeks since the premiere


def drift_at(fp: dict, m: str, w: int) -> float:
    """Market m's daily log drift in week w since the premiere: fitted by period (weeks 0-3, 4-7, 8+; a show
    fades fast after its premiere and more slowly later), else one rate."""
    seg = fp.get("drift_by_week", {}).get(m)
    if not seg:
        return fp["drift"][m]
    return seg[0] if w < DRIFT_WEEKS[0] else (seg[1] if w < DRIFT_WEEKS[1] else seg[2])


def cum_drift(fp: dict, m: str, t0: int, t1: int, premiere: int) -> float:
    """The level's total log drift from day t0 to day t1 (t1 >= t0) for a show premiering on `premiere`."""
    return sum(drift_at(fp, m, (t - premiere) // 7) for t in range(t0, t1))


def week_observation(y_mean: float, n: int, drift: float) -> float:
    """An observation that averages n daily log values describes the level about (n-1)/2 days ago: moved to
    today with the drift."""
    return y_mean + drift * (n - 1) / 2.0


def schedule(air_days: list[int], episodes: int | None, long_runner: bool) -> list[int]:
    """Air days (day numbers) of every episode: the ones known so far, then weekly until `episodes`."""
    out = sorted(air_days)
    if long_runner or episodes is None or not out:
        return out
    while len(out) < episodes:
        out.append(out[-1] + 7)
    return out


def last_air_before(sched: list[int], t: int) -> int | None:
    best = None
    for a in sched:
        if a <= t:
            best = a
        else:
            break
    return best


def att_path(fp: dict, p: dict, m: str, mean: float, var: float, today: int, sched: list[int], first_air: int,
             long_runner: bool, horizon: int, mult: float = 1.0) -> list[float]:
    """Expected attention in market m for days today+1 .. today+horizon. `sched`: every episode's air day
    (`schedule`); after the last one's week the measured post-finale decay applies (the long tail, its floor scaled
    by the show's tail prior `mult`, `tail_mult`)."""
    drift, q = fp["drift"][m], fp["q"][m]
    val = p["valuation"]
    prof, tail = post_finale_curve(fp, p, m)
    out = []
    final = sched[-1] if (sched and not long_runner) else None
    # the level drifts from today, or from the premiere for a show not yet airing (the prior is the level at
    # the premiere: before it there is nothing to fade)
    base = max(today, first_air)
    cd = 0.0   # cumulative drift from base to t (incremented day by day)
    for j in range(1, horizon + 1):
        t = today + j
        if t < first_air:
            out.append(0.0)
            continue
        if long_runner:
            out.append(math.exp(mean + 0.5 * var) * val["long_runner_decay"] ** (j / 7.0))
            continue
        if final is not None and t >= final + 7:
            # after the last episode's week: the measured post-finale decay of the level at the finale
            jf = max(0, final + 7 - base)
            lvl = math.exp(mean + cum_drift(fp, m, base, base + jf, first_air) + 0.5 * (var + q * jf)) * finale_bump(fp, m)
            tl = tail_of(fp, m)
            if tl is not None:
                # the expected long tail; the level's variance already holds the spread learned since the finale
                # week (D(now)), which the expected curve counts from the finale week on
                out.append(lvl * tail_mean(tl, t - final - 7, mult) * math.exp(-0.5 * tail_var(tl, today - final - 7)))
                continue
            w = (t - final - 7) // 7
            f = prof[w] if w < len(prof) else prof[-1] * tail ** (w - len(prof) + 1)
            out.append(lvl * f)
            continue
        a = last_air_before(sched, t)
        k = None if a is None else t - a
        je = t - base
        cd += drift_at(fp, m, (t - 1 - first_air) // 7) if t - 1 >= base else 0.0
        bump = finale_bump(fp, m) if (final is not None and final <= t < final + 7) else 1.0
        out.append(math.exp(mean + cd + 0.5 * (var + q * je)) * curve(fp, m, k) * bump)
    return out


def finale_bump(fp: dict, m: str) -> float:
    """Market m's attention in the finale's week relative to the level (fitted; AniList completions)."""
    return fp.get("finale_bump", {}).get(m, 1.0)


def post_finale_curve(fp: dict, p: dict, m: str) -> tuple[list[float], float]:
    """Market m's attention by week after the finale week and its weekly tail: fitted per market
    (sim/fit_valuation.py `post_finale`), else the game's `[valuation] post_finale_profile`."""
    if fp.get("post_finale", {}).get(m):
        return fp["post_finale"][m], fp["post_finale_tail"][m]
    val = p["valuation"]
    return val["post_finale_profile"], val["post_finale_tail_decay"]


def post_finale_factor(fp: dict, p: dict, m: str, final: int, t: int, mult: float = 1.0) -> float:
    """The post-finale factor of market m's level on day t for a show whose last episode aired on `final`: 1
    until a week after it, then the long tail's MEDIAN factor (the filter reads post-finale observations through
    it: the level after the finale is a random walk whose spread grows by D(t); `att_path` adds that spread back
    as the expected curve), else the profile and its tail."""
    if t < final + 7:
        return 1.0
    tl = tail_of(fp, m)
    if tl is not None:
        return tail_mean(tl, t - final - 7, mult) * math.exp(-0.5 * tail_var(tl, t - final - 7))
    prof, tail = post_finale_curve(fp, p, m)
    w = (t - final - 7) // 7
    return prof[w] if w < len(prof) else prof[-1] * tail ** (w - len(prof) + 1)


# ---- the long tail (docs/v2-series.md §4; fitted by sim/fit_valuation.py --history)


def tail_of(fp: dict, m: str) -> dict | None:
    """Market m's long-tail parameters, or None (older fits: the weekly profile)."""
    return fp.get("tail", {}).get(m)


def tail_parts(tl: dict, t: float, mult: float) -> tuple[float, float, float]:
    """The three parts of the expected post-finale factor t days after the finale's week, beyond the transition
    weeks: the floor × yearly^(t/365) and the slow excess × exp(-t/tau2), both scaled by the show's tail prior
    `mult` (the prior is fitted on the level 2-3 years out); the fast excess × exp(-t/tau)."""
    return (mult * tl["floor"] * tl["yearly"] ** (t / 365.0), mult * tl["excess2"] * math.exp(-t / tl["tau2"]),
            tl["excess"] * math.exp(-t / tl["tau"]))


def tail_formula(tl: dict, t: float, mult: float) -> float:
    """The expected post-finale factor beyond the transition weeks: floor + slow excess + fast excess."""
    a, b, c = tail_parts(tl, t, mult)
    return a + b + c


def tail_mean(tl: dict, t: int, mult: float) -> float:
    """The expected post-finale factor t days after the finale's week: the weekly transition, then the formula."""
    w = t // 7
    tr = tl["trans"]
    return tr[w] if w < len(tr) else tail_formula(tl, t, mult)


def tail_var(tl: dict, t: int) -> float:
    """D(t): the spread (log variance) across shows of the post-finale factor t days after the finale's week, the
    show-specific tail that observations reveal: a (1 - e^(-t/T)) + b t; 0 before."""
    if t <= 0:
        return 0.0
    v = tl["var"]
    return v[0] * (1.0 - math.exp(-t / v[1])) + v[2] * t


def tail_mult(fp: dict, score: float | None, ona: bool) -> float:
    """The show's prior tail level relative to the average (docs/v2-series.md §4.1): fitted on score and format
    (log-linear, centred so the average show has 1). A missing score counts as the reference score."""
    pr = fp.get("tail", {}).get("prior")
    if not pr:
        return 1.0
    x = pr["score"] * ((score - pr["score_ref"]) / 10.0 if score else 0.0) + (pr["ona"] if ona else 0.0) - pr["center"]
    return math.exp(x)


def tail_beyond(tl: dict, mult: float, t_last: int, last: float, disc: float) -> float:
    """Σ_{k>=1} disc^k × attention(t_last + k) for a path whose day t_last after the finale's week (beyond the
    transition) had expected attention `last`: each part is a geometric series (closed form)."""
    if t_last < 7 * len(tl["trans"]):
        return 0.0
    a, b, c = tail_parts(tl, t_last, mult)
    if a + b + c <= 0:
        return 0.0
    k = last / (a + b + c)
    g1 = tl["yearly"] ** (1 / 365.0) * disc
    g2 = math.exp(-1.0 / tl["tau2"]) * disc
    g3 = math.exp(-1.0 / tl["tau"]) * disc
    return k * (a * g1 / (1.0 - g1) + b * g2 / (1.0 - g2) + c * g3 / (1.0 - g3))


def discount_day(rate: float, p: dict) -> float:
    rw = (rate + p["valuation"]["equity_premium"]) / 52
    return (1 / (1 + rw)) ** (1 / 7.0)


def value_from_paths(own: dict, totals: dict, weights: dict, a_ref: float, p: dict, shares: int,
                     book_cents: int, rate: float, long_runner: bool, tails: dict | None = None) -> float:
    """Per-share value (yuan): expected dividends from the attention paths, discounted, plus cash per share.
    own/totals: market -> expected attention per future day (the same length). tails: market -> (tail parameters,
    the show's tail prior, days after the finale's week on the path's last day) for a path that ends in the long
    tail: its revenue beyond the horizon, at the last day's revenue per unit of attention, in closed form."""
    rv, co = p["revenue"], p["company"]
    disc = discount_day(rate, p)
    # all after-tax profit reaches the shareholders: the paid-out part as dividends, the retained part as the
    # company's cash (book value, returned at delisting); counting only the payout made every day's retained
    # cash a predictable rise of the value
    keep = (1 - co.get("operating_cost_ratio", 0)) * (1 - co["corp_tax"])
    h = len(own["us"])
    pv = 0.0
    f = 1.0
    last = 0.0
    for j in range(h):
        f *= disc
        o = {m: own[m][j] for m in MARKETS}
        tt = {m: totals[m][j] for m in MARKETS}
        share, _ = blended_share(o, tt, weights)
        dd = demand_day(tt["us"], a_ref, rv)
        last = dd * share * keep / shares
        pv += last * f
    if long_runner and h > 0:
        g = p["valuation"]["long_runner_decay"] ** (1 / 7.0) * disc
        pv += last * f * g / (1 - g)
    elif tails and h > 0:
        # beyond the horizon: each market's revenue per unit of attention stays at the last day's (the market
        # totals and demand are not forecast that far), the attention follows the long tail
        tt = {m: totals[m][h - 1] for m in MARKETS}
        live = [m for m in MARKETS if tt[m] > 0]
        wsum = sum(weights[m] for m in live) or 1.0
        dd = demand_day(tt["us"], a_ref, rv)
        for m in MARKETS:
            x = tails.get(m)
            if x is None or m not in live:
                continue
            tl, mult, t_last = x
            per = dd * (weights[m] / wsum) / tt[m] * keep / shares
            pv += f * per * tail_beyond(tl, mult, t_last, own[m][h - 1], disc)
    return pv + book_cents / 100 / shares


def flash_observation(fp: dict, hours: float, comments: int, prev_final: list[int]) -> tuple[float, float] | None:
    """A 热度快讯 as an observation of the cn level's log change: this episode's comments h hours after airing
    against the previous episodes' final counts scaled by the hourly curve. Returns (log surprise, noise
    variance) or None if there is too little to go on."""
    if not prev_final or hours <= 0:
        return None
    mean_prev = sum(prev_final) / len(prev_final)
    hc = fp["flash_curve"]
    i = min(len(hc) - 1, max(0, int(hours) - 1))
    expected = mean_prev * hc[i]
    if expected < fp["flash_min_expected"] or comments <= 0:
        return None
    var = fp["flash_r"][i]
    return math.log(comments / expected), var
