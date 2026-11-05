"""Shared economics for the simulation (sim/) and the game server (server/).

Moved verbatim from sim/simulate.py: data loading, attention per market, revenue shares and valuation.
"""

from __future__ import annotations

import json
import sqlite3
from collections import defaultdict
from dataclasses import dataclass, field
from datetime import date, timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent



# ---------------------------------------------------------------- data

@dataclass
class Show:
    id: int
    title: str
    title_en: str | None
    fmt: str
    episodes: int | None             # None = long-runner / unknown
    popularity_end: int
    studio: str | None
    days: list[date] = field(default_factory=list)
    popularity: list[float] = field(default_factory=list)
    in_progress: list[float] = field(default_factory=list)
    trending: list[float] = field(default_factory=list)
    score: list[float | None] = field(default_factory=list)
    aired: list[int] = field(default_factory=list)          # episodes aired as of each day
    first_air: date | None = None
    list_day: date | None = None
    long_runner: bool = False
    episodes_known: bool = True


def load_shows(db_path: Path, p: dict, start: date, end: date | None,
               upcoming_days: int = 0) -> tuple[list[Show], date]:
    db = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    latest = {}
    for mid, data in db.execute(
            "SELECT external_id, data FROM snapshots WHERE source='anilist' AND kind='media' ORDER BY day"):
        latest[int(mid)] = json.loads(data)   # later days overwrite earlier ones

    trends: dict[int, dict[date, dict]] = {}
    for mid, d, data in db.execute("SELECT media_id, date, data FROM anilist_trends"):
        trends.setdefault(mid, {})[date.fromisoformat(d)] = json.loads(data)

    max_day = max(d for t in trends.values() for d in t)
    if end is None:
        end = max_day - timedelta(days=1)    # the last day is usually partial
    gaps = find_gap_days(trends, end)

    u = p["universe"]
    shows = []
    for mid, m in latest.items():
        if m.get("format") not in u["formats"] or (m.get("popularity") or 0) < u["min_popularity"]:
            continue
        t = trends.get(mid)
        if not t:
            continue
        s = Show(
            id=mid,
            title=m["title"].get("native") or m["title"].get("romaji") or str(mid),
            title_en=m["title"].get("english") or m["title"].get("romaji"),
            fmt=m["format"],
            episodes=m.get("episodes"),
            popularity_end=m.get("popularity") or 0,
            studio=(m.get("studios", {}).get("nodes") or [{}])[0].get("name"),
        )
        _fill_grid(s, t, end, gaps)
        if not s.days:
            continue
        ep_days = sorted(d for d, n in t.items() if n.get("episode") is not None)
        sd = m.get("startDate") or {}
        start_date = date(sd["year"], sd["month"], sd["day"]) if sd.get("year") and sd.get("month") and sd.get("day") else None
        s.first_air = min([d for d in ep_days[:1]] + ([start_date] if start_date else [])) if (ep_days or start_date) else None
        if s.first_air is None:
            continue
        s.list_day = max(start, s.first_air - timedelta(days=u["ipo_lead_days"]))
        v = p["valuation"]
        s.long_runner = (end - s.first_air).days > 365 * v["long_runner_years"]
        s.episodes_known = s.episodes is not None
        if s.episodes is None and not s.long_runner:
            s.episodes = v["default_episodes"]
        if s.list_day > end + timedelta(days=upcoming_days):   # the game opens subscriptions ahead of listing
            continue
        shows.append(s)
    return shows, end


def find_gap_days(trends: dict[int, dict[date, dict]], end: date | None = None) -> set[date]:
    """Days AniList lost data for everyone (e.g. 2026-07-06/07 have no rows at all). A day is a gap when
    it has under half the usual row count, or its total trending is under 25% of the surrounding week's."""
    import statistics
    rows, trend = defaultdict(int), defaultdict(float)
    for t in trends.values():
        for d, n in t.items():
            rows[d] += 1
            trend[d] += n.get("trending") or 0
    if not rows:
        return set()
    days = sorted(rows)
    gaps = set()
    d = days[0]
    # days after the last data day (the collector has not run yet, or ran early) are gaps too,
    # so a live daily close carries the last day forward instead of seeing zero attention
    last = max(days[-1], end) if end else days[-1]
    while d <= last:
        near = [rows.get(d + timedelta(days=k), 0) for k in range(-7, 8) if k]
        near_t = [trend[d + timedelta(days=k)] for k in range(-3, 4) if k and rows.get(d + timedelta(days=k), 0)]
        if rows.get(d, 0) < 0.5 * statistics.median(near) or (near_t and trend[d] < 0.25 * statistics.median(near_t)):
            gaps.add(d)
        d += timedelta(days=1)
    return gaps


def _fill_grid(s: Show, t: dict[date, dict], end: date, gaps: set[date] = frozenset()) -> None:
    """Dense daily grid. Stocks (popularity, inProgress, score) forward-fill; trending is a flow, so a show's
    quiet day is 0 — except on market-wide gap days, where the previous day's value is carried forward."""
    first = min(t)
    pop = inprog = 0.0
    score = None
    aired = 0
    d = first
    while d <= end:
        n = t.get(d)
        if n:
            pop = n.get("popularity") or pop
            inprog = n.get("inProgress") if n.get("inProgress") is not None else inprog
            sc = n.get("averageScore")
            score = sc if sc else score               # 0 means "not enough votes yet"
            if n.get("episode") is not None:
                aired = max(aired, n["episode"])
        s.days.append(d)
        s.popularity.append(float(pop))
        s.in_progress.append(float(inprog))
        if d in gaps and s.trending:
            s.trending.append(s.trending[-1])
        else:
            s.trending.append(float(n.get("trending") or 0) if n else 0.0)
        s.score.append(score)
        s.aired.append(aired)
        d += timedelta(days=1)


def at(s: Show, series: list, d: date, default=0.0):
    i = (d - s.days[0]).days
    if i < 0:
        return default
    return series[min(i, len(series) - 1)]


# ---------------------------------------------------------------- economics

def score_mult(score: float | None, r: dict) -> float:
    if not score:
        return 1.0
    return min(r["score_mult_max"], max(r["score_mult_min"], (score / r["score_ref"]) ** r["score_power"]))


def attention(s: Show, d: date, r: dict) -> float:
    i = (d - s.days[0]).days
    if i < 0:
        return 0.0
    new_adds = max(0.0, s.popularity[i] - s.popularity[i - 1]) if i > 0 else 0.0
    a = r["w_in_progress"] * s.in_progress[i] / 7 + r["w_new_adds"] * new_adds + r["w_trending"] * s.trending[i]
    return a * score_mult(s.score[i], r)


MARKETS = ("us", "cn", "jp")


def market_att(s: Show, d: date, r: dict, markets: dict) -> dict[str, float]:
    """Attention of one show on one day in each market. us = AniList (observed daily);
    cn/jp = rebuilt weekly series from sim/reconstruct.py. Score premium (AniList) applies to all."""
    a = {"us": attention(s, d, r)}
    row = markets.get(s.id, {}).get(d.isoformat(), {})
    mult = score_mult(at(s, s.score, d, None), r)
    a["cn"] = row.get("cn", 0.0) * mult
    a["jp"] = row.get("jp", 0.0) * mult
    return a


def blended_share(own: dict[str, float], totals: dict[str, float], weights: dict) -> tuple[float, dict]:
    """Σ_m w_m * own_m/T_m, renormalised over markets that have any data. Returns (share, per-market part)."""
    live = [m for m in MARKETS if totals[m] > 0]
    wsum = sum(weights[m] for m in live) or 1.0
    parts = {m: (weights[m] / wsum) * own[m] / totals[m] if m in live else 0.0 for m in MARKETS}
    return sum(parts.values()), parts


def preair_att(s: Show, d: date, val: dict, aud: dict) -> dict[str, float]:
    """Expected daily attention per market once the show airs, from audience sizes before airing."""
    a = aud.get(s.id, {})
    return {"us": val["preair_attention_per_user"] * at(s, s.popularity, d) / 7,
            "cn": val["preair_cn_per_member"] * a.get("cn", 0) / 7,
            "jp": val["preair_jp_per_member"] * a.get("jp", 0) / 7}


def has_real_data(s: Show, d: date, val: dict) -> bool:
    """Premiered long enough ago that every market has reported (cn/jp lag the premiere by a day).
    Keyed on the premiere date, not AniList episode markers: long-runners such as Doraemon have none."""
    return (d - s.first_air).days >= val["min_days_after_premiere"]


def expected_att(s: Show, d: date, r: dict, markets: dict, val: dict, aud: dict) -> dict[str, float]:
    """What a market participant expects a show to draw: observed once it airs, else the larger of
    observed and the audience-based pre-air estimate. Pre-air shows must sit in the denominator too,
    or every unaired show looks like it owns its whole market."""
    a = market_att(s, d, r, markets)
    if has_real_data(s, d, val):
        return a
    pre = preair_att(s, d, val, aud)
    return {m: max(a[m], pre[m]) for m in MARKETS}


def fundamental(s: Show, d: date, weekly_div: float, book: float, rate: float, p: dict,
                preair_div: float, use_preair: bool = False) -> float:
    """Per-share value: discounted expected dividends + book value per share.

    Expected weekly dividend runs at the current level, falls once by `premiere_drop` if only the
    premiere has aired, then by `in_season_retention` per week until the finale, then follows the
    measured post-finale profile."""
    v = p["valuation"]
    rw = (rate + v["equity_premium"]) / 52
    disc = 1 / (1 + rw)
    aired = at(s, s.aired, d, 0)
    div = preair_div if use_preair else weekly_div
    if s.long_runner:
        g = v["long_runner_decay"]
        pv = div * g * disc / (1 - g * disc)
    else:
        remaining = max(0, s.episodes - aired)
        if not s.episodes_known and aired > 0:
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


# ---------------------------------------------------------------- simulation


def load_audiences(db_path: Path) -> dict[int, dict[str, float]]:
    """Bangumi (想看+在看+看过) and Annict watchers per AniList id, from the latest snapshot."""
    mfile = ROOT / "sim" / "matches.json"
    if not mfile.exists():
        return {}
    matches = json.loads(mfile.read_text(encoding="utf-8"))["matches"]
    db = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    bg = {int(i): json.loads(x) for i, x in db.execute(
        "SELECT external_id, data FROM snapshots WHERE source='bgm' AND kind='subject' ORDER BY day")}
    an = {int(i): json.loads(x) for i, x in db.execute(
        "SELECT external_id, data FROM snapshots WHERE source='annict' AND kind='work' ORDER BY day")}
    out = {}
    for aid, r in matches.items():
        a = {}
        if r.get("bgm") in bg:
            c = bg[r["bgm"]].get("collection") or {}
            a["cn"] = c.get("wish", 0) + c.get("doing", 0) + c.get("collect", 0)
        if r.get("annict") in an:
            a["jp"] = an[r["annict"]].get("watchersCount", 0)
        out[int(aid)] = a
    return out


def load_cn_titles(db_path: Path) -> dict[int, str]:
    """Chinese title per AniList id: a manual `title_cn` in sim/matches.json wins, else Bangumi's name_cn."""
    mfile = ROOT / "sim" / "matches.json"
    if not mfile.exists():
        return {}
    data = json.loads(mfile.read_text(encoding="utf-8"))
    matches, manual = data["matches"], data.get("manual", {})
    db = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    bg = {int(i): json.loads(x).get("name_cn") for i, x in db.execute(
        "SELECT external_id, data FROM snapshots WHERE source='bgm' AND kind='subject' ORDER BY day")}
    out = {}
    for aid, r in matches.items():
        name = (manual.get(aid) or {}).get("title_cn") or bg.get(r.get("bgm"))
        if name:
            out[int(aid)] = name.strip()
    return out
