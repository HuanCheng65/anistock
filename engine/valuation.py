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
             long_runner: bool, horizon: int) -> list[float]:
    """Expected attention in market m for days today+1 .. today+horizon. `sched`: every episode's air day
    (`schedule`); after the last one's week the measured post-finale decay applies."""
    drift, q = fp["drift"][m], fp["q"][m]
    val = p["valuation"]
    prof, tail = val["post_finale_profile"], val["post_finale_tail_decay"]
    out = []
    final = sched[-1] if (sched and not long_runner) else None
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
            jf = final + 7 - today
            lvl = math.exp(mean + drift * jf + 0.5 * (var + q * jf))
            w = (t - final - 7) // 7
            f = prof[w] if w < len(prof) else prof[-1] * tail ** (w - len(prof) + 1)
            out.append(lvl * f)
            continue
        a = last_air_before(sched, t)
        k = None if a is None else t - a
        out.append(math.exp(mean + drift * j + 0.5 * (var + q * j)) * curve(fp, m, k))
    return out


def discount_day(rate: float, p: dict) -> float:
    rw = (rate + p["valuation"]["equity_premium"]) / 52
    return (1 / (1 + rw)) ** (1 / 7.0)


def value_from_paths(own: dict, totals: dict, weights: dict, a_ref: float, p: dict, shares: int,
                     book_cents: int, rate: float, long_runner: bool) -> float:
    """Per-share value (yuan): expected dividends from the attention paths, discounted, plus cash per share.
    own/totals: market -> expected attention per future day (the same length)."""
    rv, co = p["revenue"], p["company"]
    disc = discount_day(rate, p)
    keep = (1 - co.get("operating_cost_ratio", 0)) * (1 - co["corp_tax"]) * co["payout_ratio"]
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
