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
        # the sequel option's parameters (sim/fit_sequel.py), next to the filter's
        sq = path.parent / "sequel.json"
        if p["valuation"].get("sequel_option", 0) == 1 and sq.exists() and "sequel" not in fp:
            fp["sequel"] = json.loads(sq.read_text(encoding="utf-8"))
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


def partial_observation(fp: dict, y_mean: float, n: int, drift: float) -> tuple[float, float] | None:
    """The first n days of a us episode week as an observation of the level today: their average log attention, less
    the fitted mean of (n days' average - the week's) (`partial_us`, sim/fit_valuation.py `fit_partial_week`), moved to
    today with the drift; and its extra noise variance beyond the full week's r. None without the fit (or n < 1).
    The close observes the days so far on top of the state without them and drops that again the next day, so the
    week's news is priced as it arrives and the complete week replaces it (docs/v2-valuation.md §3.2)."""
    pu = fp.get("partial_us")
    if n < 1 or not pu:
        return None
    mu, var = pu[min(n, len(pu)) - 1]
    return week_observation(y_mean - mu, n, drift), var


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
    weeks: the floor × yearly^(t/365), the slow excess × exp(-t/tau2) and the fast excess × exp(-t/tau), all scaled
    by the show's tail prior `mult` (fitted on the residuals 3-36 months after the finale)."""
    return (mult * tl["floor"] * tl["yearly"] ** (t / 365.0), mult * tl["excess2"] * math.exp(-t / tl["tau2"]),
            mult * tl["excess"] * math.exp(-t / tl["tau"]))


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


def pop_percentile(pr: dict, popularity: float | None) -> float | None:
    """The show's popularity percentile among the reference year's finales (`pop_ref`, sorted); None if unknown."""
    ref = pr.get("pop_ref") or []
    if popularity is None or not ref:
        return None
    return sum(1 for v in ref if v < popularity) / len(ref)


def tail_mult(fp: dict, score: float | None, ona: bool, popularity: float | None = None) -> float:
    """The show's prior tail level relative to the average (docs/v2-series.md §4.1): fitted on score, format and the
    popularity percentile (bottom fifth, top tenth), log-linear, centred so the average show has 1. A missing score
    counts as the reference score; an unknown popularity as the middle."""
    pr = fp.get("tail", {}).get("prior")
    if not pr:
        return 1.0
    x = pr["score"] * ((score - pr["score_ref"]) / 10.0 if score else 0.0) + (pr["ona"] if ona else 0.0)
    q = pop_percentile(pr, popularity)
    if q is not None and pr.get("pop_fifth"):
        x = x + pr["pop_fifth"][min(4, int(q * 5))]
    elif q is not None:
        x = x + (pr.get("pop_bottom", 0.0) if q < 0.2 else 0.0) + (pr.get("pop_top", 0.0) if q >= 0.9 else 0.0)
    return math.exp(x - pr["center"])


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


# ---- the sequel option (docs/v2-series.md §5; fitted by sim/fit_sequel.py)

SEASON_DAYS = 84      # a sequel season: 12 weekly episodes
SEQ_HORIZON = 3650    # announcements later than 10 years after the finale are not counted
SEQ_STEP = 30


def _sig(z: float) -> float:
    return 1.0 / (1.0 + math.exp(-z))


def sequel_z(sp: dict, score: float | None, source: str | None, prequel: bool, top_pop: bool) -> float:
    """The show's log-odds shift of every period's announcement hazard: score, source, itself a sequel, top-decile
    popularity (all public at the finale). A missing score counts as 70."""
    b = sp["beta"]
    return (b["score"] * ((score - 70.0) / 10.0 if score else 0.0) + (b["light_novel"] if source == "LIGHT_NOVEL" else 0.0)
            + (b["original"] if source == "ORIGINAL" else 0.0) + (b["sequel"] if prequel else 0.0)
            + (b["top_pop"] if top_pop else 0.0))


def gap_ratio(sp: dict, gap: float) -> float:
    """The sequel's expected in-season attention relative to its prequel's, by the gap from the prequel's finale."""
    g = sp["gap_ratio"]
    for b in g:
        if gap < b["until_days"]:
            return b["ratio"]
    return g[-1]["ratio"]


def announce_schedule(sp: dict, z: float, t_now: int) -> list[tuple[int, float]]:
    """[(days after the finale, probability)] of the announcement, from t_now days after the finale on (given no
    announcement so far): at the finale (its ± 7 days) while that is still open, then in 30-day steps with the
    period's yearly hazard turned into a constant daily rate."""
    a, per = sp["alpha"], sp["period_days"]
    out = []
    surv = 1.0
    start = t_now
    if t_now < per[0]:
        p0 = _sig(a[0] + z)
        out.append((max(t_now, 0), p0))
        surv = 1.0 - p0
        start = per[0]
    s = start
    while s < SEQ_HORIZON:
        k = 1 if s < per[1] else (2 if s < per[2] else (3 if s < per[3] else 4))
        lam = -math.log(1.0 - _sig(a[k] + z)) / 365.0
        q = 1.0 - math.exp(-lam * SEQ_STEP)
        out.append((s + SEQ_STEP // 2, surv * q))
        surv = surv * (1.0 - q)
        s += SEQ_STEP
    return out


def season_value(fp: dict, m: str, disc: float) -> float:
    """W_m: the present value at its premiere, in attention-days per unit of in-season mean attention, of a sequel
    season in market m: 12 weeks at the in-season mean, then its long tail from the finale week's level (the
    in-season mean × `season_to_finale`), to infinity."""
    sp = fp["sequel"]
    w = 0.0
    f = 1.0
    for _ in range(SEASON_DAYS):
        w += f
        f *= disc
    tl = tail_of(fp, m)
    if tl is None:
        return w
    c = sp["season_to_finale"][m]
    last = 0.0
    for t in range(400):
        last = c * tail_mean(tl, t, 1.0)
        w += f * last
        f *= disc
    return w + f / disc * tail_beyond(tl, 1.0, 399, last, disc)


def sequel_multiple(fp: dict, m: str, z: float, zs: float, t_now: int, disc: float, known: list[int] | None = None,
                    watched: bool = True) -> float:
    """The sequel option of a show in market m, in attention-days per unit of the show's in-season mean attention,
    discounted to today (t_now days after its finale; negative before it): Σ over announcement times and the
    announcement-to-premiere lags of P × disc^(days to the premiere) × the sequel's attention ratio × (the sequel
    season's value + its own option). z: the show's hazard shift (`sequel_z`); zs: the sequel's (itself a sequel).
    `known`: the premiere day(s) (days after the finale) of an announced sequel, equally likely. `watched`: whether
    the show's relations are collected; if not, "no announcement so far" is not information, and the option keeps
    its value as of the finale (no update when the finale passes)."""
    sp = fp["sequel"]
    w = season_value(fp, m, disc)
    # the sequel's own option at its premiere, as a fraction of its own season's value: geometric
    a_s = _option_factor(sp, zs, 0, disc) * disc ** SEASON_DAYS
    own = a_s * w / (1.0 - a_s) if a_s < 1.0 else 0.0
    if known:
        acc = 0.0
        for d in known:
            acc += disc ** max(0, d - t_now) * gap_ratio(sp, d)
        return acc / len(known) * (w + own)
    return _option_factor(sp, z, t_now, disc, t_now if watched else min(t_now, 0)) * (w + own)


def finale_level(fp: dict, m: str, mean: float, var: float, today: int, final: int, first_air: int) -> float:
    """Market m's expected attention in the finale's week (level × bump), as in `att_path`."""
    base = max(today, first_air)
    jf = max(0, final + 7 - base)
    return math.exp(mean + cum_drift(fp, m, base, base + jf, first_air) + 0.5 * (var + fp["q"][m] * jf)) * finale_bump(fp, m)


def sequel_extra(fp: dict, m: str, mean: float, var: float, today: int, final: int, first_air: int, z: float,
                 zs: float, disc: float, known: list[int] | None = None, watched: bool = True) -> float:
    """The sequel option of market m in attention-days discounted to today: the show's in-season mean attention
    (finale week ÷ `season_to_finale`) × `sequel_multiple`. `known`: an announced sequel's premiere day(s)."""
    sp = fp["sequel"]
    lvl = finale_level(fp, m, mean, var, today, final, first_air) / sp["season_to_finale"][m]
    k = [d - final for d in known] if known else None
    return lvl * sequel_multiple(fp, m, z, zs, today - final, disc, k, watched)


def _option_factor(sp: dict, z: float, t_now: int, disc: float, t_from: int | None = None) -> float:
    """Σ P(announcement) × mean over lags of disc^(days from today to the premiere) × attention ratio, with the
    announcement schedule conditioned on nothing so far as of `t_from` (default today)."""
    lags = sp["lag_days"]
    acc = 0.0
    for ta, p in announce_schedule(sp, z, t_now if t_from is None else t_from):
        s = 0.0
        for lag in lags:
            d = ta + lag
            s += disc ** max(0, d - t_now) * gap_ratio(sp, d)
        acc += p * s / len(lags)
    return acc
def discount_day(rate: float, p: dict) -> float:
    rw = (rate + p["valuation"]["equity_premium"]) / 52
    return (1 / (1 + rw)) ** (1 / 7.0)


def absolute_parts(own: dict, platform: dict, scale: float, weights: dict, rv: dict) -> dict:
    """Absolute revenue (docs/v2-series.md §6): each market's revenue per day of a show with attention `own`,
    base/7 × scale × w_m × own_m / platform_m. The platform size is each market's trailing-year total attention of
    the airing works (engine/daily_inputs.py `platform_size`); `scale` was set once, at the switch, so that the day's
    total revenue was unchanged. No renormalisation: a market a show is not mapped to earns nothing."""
    return {m: rv["demand_base_week"] / 7 * scale * weights[m] * own[m] / platform[m] if platform[m] > 0 else 0.0
            for m in MARKETS}


def value_from_paths(own: dict, totals: dict, weights: dict, a_ref: float, p: dict, shares: int,
                     book_cents: int, rate: float, long_runner: bool, tails: dict | None = None,
                     platform: tuple | None = None, extra: dict | None = None) -> float:
    """Per-share value (yuan): expected dividends from the attention paths, discounted, plus cash per share.
    own/totals: market -> expected attention per future day (the same length). tails: market -> (tail parameters,
    the show's tail prior, days after the finale's week on the path's last day) for a path that ends in the long
    tail: its revenue beyond the horizon, in closed form. platform: (platform sizes, scale[, first day index under
    the rule]) under the absolute
    revenue rule (the platform expected to stay at today's size; totals unused), else the relative rule (a share of
    the demand index, at the last day's revenue per unit of attention beyond the horizon)."""
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
    sw = platform[2] if platform is not None and len(platform) > 2 else 0
    frozen = None   # a scheduled switch: the revenue per unit of attention on the switch day, held from then on
    for j in range(h):
        f *= disc
        o = {m: own[m][j] for m in MARKETS}
        if platform is not None and sw == 0:
            parts = absolute_parts(o, platform[0], platform[1], weights, rv)
            last = (parts["us"] + parts["cn"] + parts["jp"]) * keep / shares
        elif platform is not None and j >= sw:
            # the switch's scale makes the switch day's revenue what the relative rule gives, and the absolute rule
            # then keeps each market's revenue per unit of attention (the platform expected to stay as it is): so
            # from the switch day on, the relative rule's per-unit revenue of that day, held
            if frozen is None:
                tt = {m: totals[m][j] for m in MARKETS}
                live = [m for m in MARKETS if tt[m] > 0]
                wsum = sum(weights[m] for m in live) or 1.0
                dd = demand_day(tt["us"], a_ref, rv)
                frozen = {m: dd * (weights[m] / wsum) / tt[m] if m in live else 0.0 for m in MARKETS}
            last = (frozen["us"] * o["us"] + frozen["cn"] * o["cn"] + frozen["jp"] * o["jp"]) * keep / shares
        else:
            tt = {m: totals[m][j] for m in MARKETS}
            share, _ = blended_share(o, tt, weights)
            dd = demand_day(tt["us"], a_ref, rv)
            last = dd * share * keep / shares
        pv += last * f
    if long_runner and h > 0:
        g = p["valuation"]["long_runner_decay"] ** (1 / 7.0) * disc
        pv += last * f * g / (1 - g)
    elif (tails or extra) and h > 0:
        # beyond the horizon: the attention follows the long tail, each market's revenue per unit of attention
        # as on the last day; `extra`: attention-days already discounted to today (the sequel option), valued at the
        # same revenue per unit of attention
        if platform is None or (sw > 0 and frozen is None):
            tt = {m: totals[m][h - 1] for m in MARKETS}
            live = [m for m in MARKETS if tt[m] > 0]
            wsum = sum(weights[m] for m in live) or 1.0
            dd = demand_day(tt["us"], a_ref, rv)
        for m in MARKETS:
            if platform is not None and sw == 0:
                if platform[0][m] <= 0:
                    continue
                per = rv["demand_base_week"] / 7 * platform[1] * weights[m] / platform[0][m] * keep / shares
            elif frozen is not None:
                if frozen[m] <= 0:
                    continue
                per = frozen[m] * keep / shares
            else:
                if m not in live:
                    continue
                per = dd * (weights[m] / wsum) / tt[m] * keep / shares
            x = tails.get(m) if tails else None
            if x is not None:
                tl, mult, t_last = x
                pv += f * per * tail_beyond(tl, mult, t_last, own[m][h - 1], disc)
            if extra:
                pv += per * extra[m]
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
