"""Shared economics for the simulation (sim/) and the game server (server/).

Moved verbatim from sim/simulate.py: data loading, attention per market, revenue shares and valuation.
The pure formulas (blended share, fundamental value) live in engine/valuation.py; the wrappers here
adapt them to Show objects.
"""

from __future__ import annotations

import json
import re
import os
import sqlite3
from collections import defaultdict
from dataclasses import dataclass, field
from datetime import date, timedelta
from pathlib import Path

from engine import valuation as V

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
    adult: bool = False              # AniList isAdult or the Hentai genre: never listed
    promo: bool = False              # a PV / CM / teaser compilation, not a show (`is_promo`): never listed
    source: str | None = None        # AniList source (MANGA, LIGHT_NOVEL, ORIGINAL, ...)
    genres: list[str] = field(default_factory=list)  # AniList genres + THEME_TAGS it carries (the theme indices)


# AniList tags the theme indices select on (docs/design/funds.md §3), with the rank (how central the tag is to the
# show, 0-100) from which a show counts
THEME_TAGS = {"Isekai": 60}


def show_genres(m: dict) -> list[str]:
    """AniList genres, then the theme tags the show carries at their rank or above."""
    tags = [t["name"] for t in (m.get("tags") or []) if t.get("name") in THEME_TAGS and (t.get("rank") or 0) >= THEME_TAGS[t["name"]]]
    return list(m.get("genres") or []) + sorted(tags)


# promotional material catalogued as an "anime": PVs, CMs, teasers, trailers (as a word, so "CMYK" or a
# romaji word containing "pv" does not match)
PROMO_RE = re.compile(r"(?i)(?<![a-z])(pvs?|cms?|teasers?|trailers?)(?![a-z])|ティザー|プロモーション|予告|特報|告知映像|宣伝映像")


def is_promo(mid: int, m: dict) -> bool:
    """A PV / CM / teaser entry, not a show: its title (native, romaji, English) or a synonym names it as
    one, or sim/matches.json lists it under "exclude" with reason "promo"."""
    titles = [(m.get("title") or {}).get(k) or "" for k in ("native", "romaji", "english")] + list(m.get("synonyms") or [])
    if any(PROMO_RE.search(t) for t in titles):
        return True
    return manual_excludes().get(str(mid)) == "promo"


_EXCLUDES: dict | None = None


def manual_excludes() -> dict[str, str]:
    """sim/matches.json "exclude": {AniList id: reason} for entries the rule cannot see."""
    global _EXCLUDES
    if _EXCLUDES is None:
        f = ROOT / "sim" / "matches.json"
        _EXCLUDES = json.loads(f.read_text(encoding="utf-8")).get("exclude", {}) if f.exists() else {}
    return _EXCLUDES


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
            adult=bool(m.get("isAdult")) or "Hentai" in (m.get("genres") or []),
            promo=is_promo(mid, m),
            source=m.get("source"),
            genres=show_genres(m),
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
        _extend_into_gap(s, gaps)
        s.list_day = max(start, s.first_air - timedelta(days=u["ipo_lead_days"]))
        v = p["valuation"]
        # a long-runner has been on air for years: its run so far, to its end date once AniList calls it finished (a show
        # that premiered years ago and ran one season is not one: a listed sequel's finished prequel, docs/design/series.md §13)
        ed = m.get("endDate") or {}
        run_end = end
        if m.get("status") == "FINISHED" and ed.get("year") and ed.get("month"):
            run_end = min(end, date(ed["year"], ed["month"], ed.get("day") or 28))
        s.long_runner = (run_end - s.first_air).days > 365 * v["long_runner_years"]
        s.episodes_known = s.episodes is not None
        if s.episodes is None and not s.long_runner:
            s.episodes = v["default_episodes"]
        if s.list_day > end + timedelta(days=upcoming_days):   # the game opens subscriptions ahead of listing
            continue
        shows.append(s)
    return shows, end


GAP_BASELINE = range(20, 48)   # days before a day whose row counts give its usual count
GAP_TREND_SHARE = 0.5          # a day whose total trending is under this share of the surrounding week's (a normal
                               # weekly low is about 60%; 2026-03-31, partial, 43%)


def find_gap_days(trends: dict[int, dict[date, dict]], end: date | None = None) -> set[date]:
    """Days AniList lost data for everyone. AniList has no trend rows at all on 2026-07-06/07 and 2026-04-02..18 (its
    daily trend job stopped while the site kept working: list entries grew as usual); 07-08 and 04-01 are partial
    days (and 03-31, 43% of the week around it). A day is a gap when it has under half the usual row count, or its
    total trending is under half the surrounding week's. The usual count is the median over 20–47 days earlier, so a gap of any length is seen (a
    window around the day, as before, sees only empty days inside a long gap); near the start of the data, where
    that window is mostly empty, the surrounding two weeks."""
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
        base = [rows[d - timedelta(days=k)] for k in GAP_BASELINE if rows.get(d - timedelta(days=k), 0)]
        if len(base) < 7:
            base = [rows.get(d + timedelta(days=k), 0) for k in range(-7, 8) if k]
        near_t = [trend[d + timedelta(days=k)] for k in range(-3, 4) if k and rows.get(d + timedelta(days=k), 0)]
        if rows.get(d, 0) < 0.5 * statistics.median(base) or (near_t and trend[d] < GAP_TREND_SHARE * statistics.median(near_t)):
            gaps.add(d)
        d += timedelta(days=1)
    return gaps


def _fill_grid(s: Show, t: dict[date, dict], end: date, gaps: set[date] = frozenset()) -> None:
    """Dense daily grid. Stocks (popularity, inProgress, score) forward-fill; trending is a flow, so a show's
    quiet day is 0. On market-wide gap days (find_gap_days) nothing was measured: a run of them between two measured
    days is drawn straight between those days, for trending, popularity, inProgress and the episodes aired (so the
    list adds of the gap are spread over it instead of landing on the first day back, and a show airing through the
    gap stays airing); a run with no measured day after it (the latest days, live) carries the last day forward."""
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
    _bridge_gaps(s, gaps)


def _extend_into_gap(s: Show, gaps: set[date]) -> None:
    """A show that premiered inside a market-wide gap has no rows until the gap ends (most of the spring 2026
    premieres: AniList's first rows are 2026-04-19). Its grid then starts at its premiere: the days back to the
    premiere take the first measured day's trending, popularity, inProgress and score, and the episodes rise straight
    from none to the first measured count, so the show is airing through the gap. List adds over those days are not
    known (counted as none)."""
    if not s.days or s.first_air is None or s.first_air >= s.days[0]:
        return
    lead = []
    d = s.first_air
    while d < s.days[0]:
        if d not in gaps:
            return                      # a measured day without rows for this show: not a gap's doing
        lead.append(d)
        d += timedelta(days=1)
    k = len(lead)
    s.days[:0] = lead
    s.trending[:0] = [s.trending[0]] * k
    s.popularity[:0] = [s.popularity[0]] * k
    s.in_progress[:0] = [s.in_progress[0]] * k
    s.score[:0] = [s.score[0]] * k
    s.aired[:0] = [int(s.aired[k] * (i + 1) / (k + 1)) for i in range(k)]


def _bridge_gaps(s: Show, gaps: set[date]) -> None:
    """Draw each run of gap days straight between the measured days on either side (see _fill_grid)."""
    n = len(s.days)
    i = 0
    while i < n:
        if s.days[i] not in gaps:
            i += 1
            continue
        j = i
        while j < n and s.days[j] in gaps:
            j += 1
        # days i..j-1 are a gap; i-1 and j are measured (when they exist)
        if i > 0 and j < n:
            span = j - (i - 1)
            for k in range(i, j):
                f = (k - (i - 1)) / span
                for series in (s.trending, s.popularity, s.in_progress):
                    series[k] = series[i - 1] + (series[j] - series[i - 1]) * f
            # AniList books an episode count only on the day of a new episode: episodes aired in the gap show up at
            # the next one after it, so the count is drawn up to that day (within two weeks of the gap's end)
            m = next((x for x in range(j, min(n, j + 15)) if s.aired[x] > s.aired[i - 1]), None)
            if m is not None:
                for k in range(i, m):
                    s.aired[k] = int(s.aired[i - 1] + (s.aired[m] - s.aired[i - 1]) * (k - (i - 1)) / (m - (i - 1)))
        i = j


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


def raw_attention(s: Show, d: date, r: dict) -> float:
    """AniList attention on day D without the score premium: the heat index's us figure (engine/heat_index.py v2;
    the index measures heat, the premium is part of the revenue)."""
    i = (d - s.days[0]).days
    if i < 0:
        return 0.0
    new_adds = max(0.0, s.popularity[i] - s.popularity[i - 1]) if i > 0 else 0.0
    return r["w_in_progress"] * s.in_progress[i] / 7 + r["w_new_adds"] * new_adds + r["w_trending"] * s.trending[i]


def attention(s: Show, d: date, r: dict) -> float:
    """The revenue's us attention: raw attention x the score premium (`score_mult`)."""
    i = (d - s.days[0]).days
    if i < 0:
        return 0.0
    return raw_attention(s, d, r) * score_mult(s.score[i], r)


MARKETS = V.MARKETS


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
    """Σ_m w_m * own_m/T_m, renormalised over markets that have any data (engine/valuation.py)."""
    return V.blended_share(own, totals, weights)


def preair_att(s: Show, d: date, val: dict, aud: dict) -> dict[str, float]:
    """Expected daily attention per market once the show airs, from audience sizes before airing."""
    a = aud.get(s.id, {})
    return {"us": val["preair_attention_per_user"] * at(s, s.popularity, d) / 7,
            "cn": val["preair_cn_per_member"] * a.get("cn", 0) / 7,
            "jp": val["preair_jp_per_member"] * a.get("jp", 0) / 7}


def preair_pit(s: Show, d: date, val: dict, aud_dated: dict) -> dict[str, float]:
    """Point-in-time pre-air estimate of daily attention (the valuation filter's prior), frozen at the premiere:
    only data that existed on min(D, premiere - 1). us from the AniList list entries on that day; cn/jp only
    from a Bangumi/Annict audience snapshot taken before the premiere (0 = none: the filter then derives
    cn/jp from the us estimate with its fitted per-market factors)."""
    df = min(d, s.first_air - timedelta(days=1))
    out = {"us": val["preair_attention_per_user"] * at(s, s.popularity, df) / 7, "cn": 0.0, "jp": 0.0}
    a = aud_dated.get(s.id, {})
    for m, k in (("cn", "preair_cn_per_member"), ("jp", "preair_jp_per_member")):
        before = [count for count, snap_day in a.get(m, []) if snap_day <= df]
        if before:
            out[m] = val[k] * before[-1] / 7
    return out


def observed_markets(s: Show, d: date, markets: dict) -> dict[str, bool]:
    """Whether each market reported on D (us always; cn/jp: a value in the rebuilt series, 0 included)."""
    row = markets.get(s.id, {}).get(d.isoformat(), {})
    return {"us": True, "cn": "cn" in row, "jp": "jp" in row}


def has_real_data(s: Show, d: date, val: dict) -> bool:
    """Premiered long enough ago that every market has reported (cn/jp lag the premiere by a day).
    Keyed on the premiere date, not AniList episode markers: long-runners such as Doraemon have none."""
    return V.has_real_data((d - s.first_air).days, val)


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
    """Per-share value: discounted expected dividends + book value per share (engine/valuation.py)."""
    return V.fundamental(at(s, s.aired, d, 0), s.episodes, s.episodes_known, s.long_runner,
                         weekly_div, book, rate, p, preair_div, use_preair=use_preair)


# ---------------------------------------------------------------- simulation


SEASON_MONTH = {"WINTER": 1, "SPRING": 4, "SUMMER": 7, "AUTUMN": 10}
# a show premiering late in a season (or a few weeks early) is still within this of its Annict season's first month
MAL_SEASON_MONTHS = 4


def effective_matches(db_path: Path | None = None) -> dict[str, dict]:
    """AniList id (str) -> {"bgm": id, "annict": id, ...}: sim/matches.json, plus its reviewed Bangumi archive matches
    ("archive", `bgm_via` "archive") for shows it gives no subject, plus an Annict work for every
    AniList show without one whose MAL id an Annict work in the collector carries (the `works` table, where
    the collector registers works it discovers by title, and the Annict work snapshots), plus the Bangumi subject the
    collector matched for a show sim/matches.json has none for (`bgm_matches`, engine/bgm_match.py), plus the
    `manual` corrections, which win. So the server picks up newly collected works without re-running sim/match.py.

    A MAL id join is skipped when the Annict work's season starts more than `MAL_SEASON_MONTHS` from the AniList
    show's premiere: MyAnimeList keeps some split cours as one entry, so AniList gives the later part the earlier
    part's MAL id (JoJo SBR 2nd & 3rd STAGE, 2026-09, carries 1st STAGE's 61469 and was joined to its Annict work
    of winter 2026)."""
    data = json.loads((ROOT / "sim" / "matches.json").read_text(encoding="utf-8"))
    out = {k: dict(v) for k, v in data["matches"].items()}
    # the reviewed Bangumi archive matches ("archive"), for shows nothing above gives a subject; a subject another
    # show already has is left out (one subject counted under two shows would double its cn viewers)
    taken = {r["bgm"] for r in out.values() if r.get("bgm")}
    for aid, a in data.get("archive", {}).items():
        r = out.setdefault(aid, {"anilist": int(aid)})
        if not r.get("bgm") and a["bgm"] not in taken:
            r["bgm"] = a["bgm"]
            r["bgm_via"] = "archive"
            taken.add(a["bgm"])
    path = db_path or Path(os.environ.get("ANISTOCK_DB", ROOT / "data" / "anistock.db"))
    if path.exists():
        db = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
        try:
            mal_of: dict[str, str] = {}
            annict_of_mal: dict[str, int] = {}
            start_of: dict[str, int] = {}      # AniList id -> premiere, in months (year * 12 + month)
            season_of: dict[int, int] = {}     # Annict id -> its season's first month, the same way
            tables = {r[0] for r in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
            if "works" in tables:
                for src, ext, mal in db.execute("SELECT source, external_id, mal_id FROM works WHERE mal_id IS NOT NULL AND mal_id != ''"):
                    if src == "anilist":
                        mal_of[str(ext)] = str(mal)
                    elif src == "annict":
                        annict_of_mal.setdefault(str(mal), int(ext))
            for ext, x in db.execute("SELECT external_id, data FROM snapshots WHERE source='anilist' AND kind='media' ORDER BY day"):
                m = json.loads(x)
                if m.get("idMal"):
                    mal_of[str(ext)] = str(m["idMal"])
                sd = m.get("startDate") or {}
                if sd.get("year"):
                    start_of[str(ext)] = sd["year"] * 12 + (sd.get("month") or 1)
            for ext, x in db.execute("SELECT external_id, data FROM snapshots WHERE source='annict' AND kind='work' ORDER BY day"):
                w = json.loads(x)
                if w.get("malAnimeId"):
                    annict_of_mal.setdefault(str(w["malAnimeId"]), int(ext))
                if w.get("seasonYear") and w.get("seasonName") in SEASON_MONTH:
                    season_of[int(ext)] = w["seasonYear"] * 12 + SEASON_MONTH[w["seasonName"]]
            from engine.bgm_match import table as bgm_table
            for aid, sid in bgm_table(db).items():
                r = out.setdefault(aid, {"anilist": int(aid)})
                if not r.get("bgm"):
                    r["bgm"] = sid
                    r["bgm_via"] = "collector"
        finally:
            db.close()
        for aid, mal in mal_of.items():
            r = out.setdefault(aid, {"anilist": int(aid)})
            if not r.get("annict") and mal in annict_of_mal:
                wid = annict_of_mal[mal]
                if aid in start_of and wid in season_of and abs(start_of[aid] - season_of[wid]) > MAL_SEASON_MONTHS:
                    continue
                r["annict"] = wid
                r["annict_via"] = "mal_id"
    for k, v in data.get("manual", {}).items():
        r = out.setdefault(k, {"anilist": int(k)})
        for key in ("bgm", "annict"):
            if key in v:
                r[key] = v[key]
    return out


def load_audiences(db_path: Path) -> dict[int, dict[str, float]]:
    """Bangumi (想看+在看+看过) and Annict watchers per AniList id, from the latest snapshot."""
    if not (ROOT / "sim" / "matches.json").exists():
        return {}
    matches = effective_matches(db_path)
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


def load_audiences_dated(db_path: Path) -> dict[int, dict[str, list[tuple[float, date]]]]:
    """Like load_audiences, with every snapshot and its day, oldest first: {AniList id: {"cn": [(count, day),
    ...], "jp": [...]}} (preair_pit takes the last one before the premiere)."""
    if not (ROOT / "sim" / "matches.json").exists():
        return {}
    matches = effective_matches(db_path)
    db = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    bg: dict[int, list] = {}
    an: dict[int, list] = {}
    for i, day, x in db.execute("SELECT external_id, day, data FROM snapshots WHERE source='bgm' AND kind='subject' ORDER BY day"):
        c = json.loads(x).get("collection") or {}
        bg.setdefault(int(i), []).append((c.get("wish", 0) + c.get("doing", 0) + c.get("collect", 0), date.fromisoformat(day)))
    for i, day, x in db.execute("SELECT external_id, day, data FROM snapshots WHERE source='annict' AND kind='work' ORDER BY day"):
        an.setdefault(int(i), []).append((json.loads(x).get("watchersCount", 0), date.fromisoformat(day)))
    db.close()
    out = {}
    for aid, r in matches.items():
        a = {}
        if r.get("bgm") in bg:
            a["cn"] = bg[r["bgm"]]
        if r.get("annict") in an:
            a["jp"] = an[r["annict"]]
        out[int(aid)] = a
    return out


def mapped_sources(db_path: Path | None = None) -> dict[int, dict[str, bool]]:
    """Which market data sources a show is mapped to (`effective_matches`): cn = Bangumi, jp = Annict."""
    if not (ROOT / "sim" / "matches.json").exists():
        return {}
    return {int(k): {"cn": bool(v.get("bgm")), "jp": bool(v.get("annict"))} for k, v in effective_matches(db_path).items()}


def load_cn_titles(db_path: Path) -> dict[int, str]:
    """Chinese title per AniList id: a manual `title_cn` in sim/matches.json wins, else the name_cn of the Bangumi
    subject `effective_matches` gives it."""
    mfile = ROOT / "sim" / "matches.json"
    if not mfile.exists():
        return {}
    data = json.loads(mfile.read_text(encoding="utf-8"))
    matches, manual = effective_matches(db_path), data.get("manual", {})
    db = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    bg = {int(i): json.loads(x).get("name_cn") for i, x in db.execute(
        "SELECT external_id, data FROM snapshots WHERE source='bgm' AND kind='subject' ORDER BY day")}
    out = {}
    for aid, r in matches.items():
        name = (manual.get(aid) or {}).get("title_cn") or bg.get(r.get("bgm"))
        if name:
            out[int(aid)] = name.strip()
    return out
