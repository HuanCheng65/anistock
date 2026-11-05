"""The pure formulas that turn the model's daily inputs (engine/daily_inputs.py) plus game state into
revenue shares and per-share values. No I/O, no Show objects: plain numbers and dicts.

This is the part the Rust game server ports (docs/v2-architecture.md §1). The simulation (sim/) uses
the same functions through engine/economy.py, so there is one copy of each formula. Every float
operation is written in a fixed order; a port must keep that order (and the summation order of its
callers) to reproduce the Python results bit for bit.
"""

from __future__ import annotations

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
