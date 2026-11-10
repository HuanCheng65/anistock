"""The economic model's output for one game day: the interface between the Python model and the game server.

    uv run engine/daily_inputs.py --day 2026-11-05 [--out PATH] [--window-start 2026-07-01]

Writes data/derived/inputs/<day>.json (or $ANISTOCK_INPUTS_DIR/<day>.json, or --out). The daily close
(server/daily.py now, the Rust server later) reads everything it needs from the model out of this file and
combines it with game state through the pure formulas in engine/valuation.py. The file is deterministic:
the same collector data, derived market series and parameters give the same bytes.

File format (JSON, one object):

    day           "YYYY-MM-DD"  the day being closed (D)
    window_start  "YYYY-MM-DD"  the game's data window start (game state `window_start`)
    cn_titles     {anilist_id: name}  Chinese title for every matched show (not only those in `shows`): a manual
                  `title_cn` in sim/matches.json wins, else Bangumi's name_cn. The close renames any company
                  whose id is here (refresh_titles); a new company is named cn_titles[id], else title_ja.
    platform      {"us","cn","jp"}  each market's platform size (docs/design/series.md §6): the mean, over the trailing
                  year's days with data, of the day's total attention of every tracked work airing (an episode in
                  the last 14 days; listed or not; adult titles and promos aside) (phase 1)
    platform_days the number of days `platform` averages (up to 365; the close switches to absolute revenue once
                  it reaches revenue.platform_min_days) (phase 1)
    shows         [ ... ]  every show of the model's universe that is airing or lists within
                  ipo_subscription_days + 1 days of D, IN MODEL ORDER (new IPOs are created, and their stock
                  codes numbered, in this order). Each entry:
        id              AniList id (= company id)
        title_ja        native title (else romaji, else the id)
        title_en        English title (else romaji); may be null
        studio          main studio; may be null
        first_air       "YYYY-MM-DD"  premiere date
        episodes        episode count used by the valuation; when unknown and not a long-runner, the default
                        count (valuation.default_episodes); null only for long-runners with no count
        episodes_known  false when `episodes` is the default (companies store null then)
        long_runner     premiered more than valuation.long_runner_years before D
        popularity_end  AniList list entries in the latest snapshot (gate: game.min_listing_popularity)
        aired           episodes aired as of D (0 before the premiere)
        has_real_data   D is at least valuation.min_days_after_premiere days after first_air
        air_dates       ["YYYY-MM-DD", ...] the day each episode aired so far (one entry per episode; since v0.3.0)
        adult           true for an adult title (AniList isAdult or the Hentai genre); absent otherwise.
                        Never listed; a listed one is bought out at the next close (since v0.3.1)
        promo           true for a PV / CM / teaser entry (engine/economy.py is_promo); absent otherwise. Treated
                        like `adult` (since v0.3.2)
        preair          {"us","cn","jp"}  the audience-based pre-air estimate of daily attention per market, from
                        the LATEST audience snapshot (look-ahead for shows already airing; since v0.3.0, not used
                        by the valuation since v0.3.2)
        prior_prev      {"us","cn","jp"}  `prior` 7 days earlier, before the premiere (absent when the data does not
                        reach back): the show's own growth (valuation `preair_trend`)
        prior           {"us","cn","jp"}  point-in-time pre-air estimate, frozen at the premiere: us from AniList
                        list entries on min(D, premiere-1); cn/jp only from a Bangumi/Annict snapshot taken by then,
                        else 0 (the filter derives them from us). The valuation filter's prior (since v0.3.2)
        observed        {"us","cn","jp"}  whether the market reported on D (cn/jp: a value in the rebuilt weekly
                        series, 0 included; false = no data, not zero) (since v0.3.2)
        sources         {"us","cn","jp"}  whether the show is mapped to the market's data source (Bangumi, Annict
                        in sim/matches.json); an unmapped market earns nothing (since v0.3.2). Point in time
                        (v0.6.1): a source counts from the first day the collector had the mapped Bangumi subject /
                        Annict work (`source_dates`), so a history regenerated later agrees with what the daily
                        close saw on each day
        att             {"us","cn","jp"}  realised attention on D per market (revenue shares, demand index)
        score           AniList averageScore on D (null: not enough votes yet); the long tail's prior (phase 1)
        format          AniList format (TV, ONA, MOVIE, ...) (phase 1)
        source          AniList source (MANGA, LIGHT_NOVEL, ORIGINAL, ...) (phase 1)
        genres          AniList genres, then the theme tags the show carries (economy.THEME_TAGS: Isekai)
        prequel         itself a sequel: the relations known on D have a PREQUEL TV/ONA/film (phase 1)
        top_pop         top decile of popularity among the shows premiering within half a year of it (phase 1)
        sequel          an announced sequel not yet aired, from the relations known on D, else null (phase 1;
                        engine/daily_inputs.py `series_facts`): {"id", "announced" (the first day the relation was
                        seen), "premieres" [days, equally likely], "exact" (an announced day), "title" (the
                        entry's native title), "kind" ("season", "part": a later part of a split season, with "part" its
                        number when the title gives one, "film"), "when" (a premiere announced to the month or
                        year: "2027 年 4 月"),
                        "known_before" (already in the show's first relations snapshot: announced earlier, on a day
                        not seen), "source" (an operator's override: the official announcement)};
                        data/sequel_overrides.json applies (`apply_sequel_override`)
        popularity      AniList list entries on D (the tail prior's popularity at the finale) (phase 1)
        relations_known the collector has the show's AniList relations as of D; without them "no sequel announced
                        yet" is not information and the sequel option is not updated as time passes (phase 1)
        sequel_cancelled  the makers said officially there will be no sequel (an operator's override, its source):
                        no sequel option (v0.8.4, docs/design/series.md §5.4); absent otherwise
        schedule        ["YYYY-MM-DD", ...] every main episode's air date on Bangumi (the public schedule); a show
                        whose schedule has an episode after the last one AniList recorded is not finished (phase 1);
                        the valuation dates the episodes still to air by it (docs/design/corporate.md §2.2)
        hiatus          AniList status HIATUS on D (production halted; omitted when false) (docs/design/corporate.md §2.3)
        next_airing     AniList nextAiringEpisode on D: {"episode", "date" (Japanese broadcast date)}; omitted
                        without one (docs/design/corporate.md §2.2)
        series          the series key (engine/series.py: SEQUEL/PREQUEL among TV/ONA/films, plus sim/series.json;
                        the smallest AniList id in the series); a new season of a listed series joins its company
                        instead of listing (phase 1)
        expected        {"us","cn","jp"}  expected attention on D: `att` once the show has real data, else the
                        per-market max of `att` and the audience-based pre-air estimate (expected-share
                        denominator, IPO pricing, fair values)

Top level, besides the shows: `heat` {"<id>": {"idx", "us", "cn", "jp", "flags", "version"}}, the AniEx heat index
of D for every show engine/heat_index.py has published (docs/design/futures.md §3; frozen once written; omitted for a
show without a row). Futures settle on it. Top level so that a server binary from before it ignores it.

Not in the file, because they come from game state or config rather than the model: the demand index
(from Σ att.us over *listed* companies and the game's `a_ref`), market weights, rates, prices.
"""

from __future__ import annotations

import argparse
from collections import Counter
import json
import os
import re
import sqlite3
import sys
from dataclasses import dataclass, field
from datetime import date, datetime, timedelta, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
if str(ROOT) not in sys.path:          # run as a script: engine/ is sys.path[0], the repo root is not
    sys.path.insert(0, str(ROOT))

from engine import economy as E  # noqa: E402


def inputs_dir() -> Path:
    return Path(os.environ.get("ANISTOCK_INPUTS_DIR", ROOT / "data" / "derived" / "inputs"))


def load_markets() -> dict[int, dict]:
    mpath = Path(os.environ.get("ANISTOCK_DERIVED_DIR", ROOT / "data" / "derived")) / "markets.json"
    if not mpath.exists():
        return {}
    return {int(k): v for k, v in json.loads(mpath.read_text(encoding="utf-8"))["series"].items()}


def air_dates(s: E.Show, day: date, schedule: list[str] | None = None) -> list[str]:
    """The day each episode aired so far (from the model's per-day episode count), plus the episodes of a release
    of several at once (`batch_aired`) on the schedule's day. A series member that aired before the trends: its
    episodes' days from its run (economy.py `member_episodes`)."""
    if s.ep_dates is not None:
        return [x.isoformat() for x in s.ep_dates if x <= day]
    out, prev = [], 0
    for d, a in zip(s.days, s.aired):
        if d > day:
            break
        if a > prev:
            out.extend([d.isoformat()] * (a - prev))
        prev = a
    n, d = batch_aired(schedule or [], day, len(out))
    if n > len(out):
        out.extend([d] * (n - len(out)))
    return out


def batch_aired(schedule: list[str], day: date, aired: int) -> tuple[int, str | None]:
    """Episodes released several at once (a streaming service's whole season, a double or triple premiere): AniList
    records only the first of them (no airing schedule), so the count stays at 1 and the show looks like it is
    still airing for months. When the public schedule (Bangumi) dates two or more episodes on the same day, on or
    before `day`, and the show has aired, every episode up to that release counts as aired from its date."""
    if aired < 1:
        return aired, None
    full = sorted(d for d in schedule if len(d) == 10)     # exact dates only ("2027-04" is an announcement)
    c = Counter(full)
    out, at = aired, None
    for i, d in enumerate(full):
        if d > day.isoformat():
            break
        if c[d] >= 2 and i + 1 > out:
            out, at = i + 1, d
    return out, at


SERIES_FORMATS = {"TV", "TV_SHORT", "ONA", "MOVIE"}


def load_relations(db_path: Path) -> dict[int, list[tuple[date, list]]]:
    """AniList relations snapshots (collector kind 'relations'): {id: [(day, edges)]} oldest first."""
    db = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    out: dict[int, list] = {}
    try:
        for mid, d, data in db.execute(
                "SELECT external_id, day, data FROM snapshots WHERE source='anilist' AND kind='relations' ORDER BY day"):
            out.setdefault(int(mid), []).append((date.fromisoformat(d), json.loads(data).get("edges") or []))
    except sqlite3.Error:
        pass
    finally:
        db.close()
    return out


def _fdate(d: dict | None) -> tuple[date | None, str]:
    """An AniList fuzzy date and its precision: day, month, year or none."""
    if not d or not d.get("year"):
        return None, "none"
    if not d.get("month"):
        return date(d["year"], 7, 1), "year"
    if not d.get("day"):
        return date(d["year"], d["month"], 15), "month"
    return date(d["year"], d["month"], d["day"]), "day"


SEQUEL_ID_BASE = 1_000_000_000   # an override's sequel id: this plus the work's id (AniList ids are far below)


def sequel_overrides_path() -> Path:
    return Path(os.environ.get("ANISTOCK_SEQUEL_OVERRIDES", ROOT / "data" / "sequel_overrides.json"))


def load_sequel_overrides(path: Path | None = None) -> dict[int, dict]:
    """The operator's sequel overrides (engine/sequel_override.py; docs/design/series.md §5.4), by work id: an
    official announcement AniList has not listed yet ("announced", "premiere", "source") or an official statement that
    there will be no sequel ("cancelled", "source"). Missing file: none."""
    path = path or sequel_overrides_path()
    if not path.exists():
        return {}
    return {int(k): v for k, v in json.loads(path.read_text(encoding="utf-8")).items()}


def apply_sequel_override(facts: dict, sid: int, ov: dict | None, day: date, lags: list[int]) -> dict:
    """`series_facts` with the work's override as of `day`. An announcement keeps one sequel id from the override's
    day on, also once AniList lists the entry (its dates are used then), so the notice and the production cost come
    once; a statement that there will be no sequel drops the sequel and sets `sequel_cancelled` to its source."""
    out = dict(facts)
    if not ov:
        return out
    if ov.get("cancelled") and date.fromisoformat(ov["cancelled"]) <= day:
        out["sequel"] = None
        out["sequel_cancelled"] = ov.get("source") or ""
        return out
    if ov.get("announced") and date.fromisoformat(ov["announced"]) <= day:
        a = date.fromisoformat(ov["announced"])
        q = out.get("sequel")
        if q is None:
            start, prec = _fdate(dict(zip(("year", "month", "day"), map(int, ov["premiere"].split("-"))))
                                 if ov.get("premiere") else None)
            if prec == "none":
                prem = [a + timedelta(days=lag) for lag in lags]
            else:
                prem = [max(start, day + timedelta(days=1))]
            if not prem:
                return out
            q = {"premieres": [d.isoformat() for d in prem], "exact": prec == "day"}
        out["sequel"] = {**q, "id": SEQUEL_ID_BASE + sid, "announced": a.isoformat(), "source": ov.get("source")}
        out["relations_known"] = True
    return out


PART_WORDS = ("クール", "cour", "Cour", "Part", "PART", "パート", "部分")
PART_NUMBER = re.compile(r"第\s*([0-9０-９]+|[IVX]+|[ⅠⅡⅢⅣⅤⅥ])\s*(?:クール|部分|部)|(?:Part|PART|パート)\s*([0-9０-９]+|[IVX]+)"
                         r"|([0-9]+)(?:st|nd|rd|th)\s*[Cc]our")
ROMAN = {"I": 1, "II": 2, "III": 3, "IV": 4, "V": 5, "VI": 6, "Ⅰ": 1, "Ⅱ": 2, "Ⅲ": 3, "Ⅳ": 4, "Ⅴ": 5, "Ⅵ": 6}


def sequel_kind(node: dict) -> tuple[str, int | None]:
    """How a SEQUEL entry continues the show, for its notice: "film", "part" (a later part of a split season:
    無職転生Ⅲ 第2クール) with its number when the title gives one, else "season"."""
    if node.get("format") == "MOVIE":
        return "film", None
    t = node.get("title") or {}
    names = " ".join(x for x in (t.get("native"), t.get("romaji"), t.get("english")) if x)
    if not any(w in names for w in PART_WORDS):
        return "season", None
    m = PART_NUMBER.search(names)
    n = next((g for g in m.groups() if g), None) if m else None
    if n is None:
        return "part", None
    return "part", ROMAN.get(n) or int(n.translate(str.maketrans("０１２３４５６７８９", "0123456789")))


def series_facts(sid: int, rel: dict, day: date, lags: list[int]) -> dict:
    """What the relations known on `day` say (docs/design/series.md §5): whether the show is itself a sequel (a PREQUEL
    TV/ONA/film), and an announced sequel that has not aired: a SEQUEL TV/ONA/film entry not yet released, announced
    on the first day the relation was seen, premiering on its start date (a month or year only: the middle of it),
    else the announcement plus each fitted announcement-to-premiere lag."""
    snaps = [(d, e) for d, e in rel.get(sid, []) if d <= day]
    if not snaps:
        return {"prequel": False, "sequel": None, "relations_known": False}
    edges = snaps[-1][1]

    def anime(e):
        return e["node"].get("type") == "ANIME" and e["node"].get("format") in SERIES_FORMATS
    prequel = any(e["relationType"] == "PREQUEL" and anime(e) for e in edges)
    cands = []
    for e in edges:
        n = e["node"]
        if e["relationType"] != "SEQUEL" or not anime(e):
            continue
        start, prec = _fdate(n.get("startDate"))
        if n.get("status") not in ("NOT_YET_RELEASED", None) or (start is not None and prec == "day" and start <= day):
            continue
        first = min(d for d, es in snaps if any(x["node"]["id"] == n["id"] and x["relationType"] == "SEQUEL" for x in es))
        # already there in the show's first relations snapshot: announced on some earlier day we did not see
        before = first == snaps[0][0]
        if prec == "none":
            prem = [first + timedelta(days=lag) for lag in lags]
        else:
            prem = [max(start, day + timedelta(days=1))]
        if not prem:    # no date and no fitted lags (the sequel option is off): nothing to say about when
            continue
        t = n.get("title") or {}
        kind, part = sequel_kind(n)
        hint = {"month": f"{start.year} 年 {start.month} 月", "year": f"{start.year} 年"}.get(prec) if start else None
        cands.append((min(prem), {"id": n["id"], "announced": first.isoformat(), "premieres": [d.isoformat() for d in prem],
                                  "exact": prec == "day", "title": t.get("native") or t.get("romaji"),
                                  "kind": kind, **({"part": part} if part else {}),
                                  **({"when": hint} if hint else {}), "known_before": before}))
    cands.sort(key=lambda c: (c[0], c[1]["id"]))
    return {"prequel": prequel, "sequel": cands[0][1] if cands else None, "relations_known": True}


def load_schedules(db_path: Path) -> dict[int, list[tuple[date, list[str]]]]:
    """Bangumi's episode lists (collector kind 'episodes'): {AniList id: [(snapshot day, sorted air dates)]}, mapped
    through sim/matches.json. Air dates are a published schedule: an episode's date is public before it airs."""
    bg = {int(r["bgm"]): int(k) for k, r in E.effective_matches(db_path).items() if r.get("bgm")}
    db = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    out: dict[int, list] = {}
    try:
        for sid, d, data in db.execute(
                "SELECT external_id, day, data FROM snapshots WHERE source='bgm' AND kind='episodes' ORDER BY day"):
            aid = bg.get(int(sid))
            if aid is None:
                continue
            dates = sorted(e["airdate"] for e in json.loads(data).get("data", []) if e.get("airdate"))
            out.setdefault(aid, []).append((date.fromisoformat(d), dates))
    finally:
        db.close()
    return out


JST = timezone(timedelta(hours=9))


def load_airing(db_path: Path) -> dict[int, list[tuple[date, str | None, dict | None]]]:
    """AniList's status and next episode per daily media snapshot (docs/design/corporate.md §2.2–2.3):
    {id: [(day, status, {"episode": n, "date": "YYYY-MM-DD"} or None)]}, oldest first. The next episode's day is the
    Japanese broadcast date of `airingAt`."""
    db = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    out: dict[int, list] = {}
    try:
        for mid, d, data in db.execute(
                "SELECT external_id, day, data FROM snapshots WHERE source='anilist' AND kind='media' ORDER BY day"):
            m = json.loads(data)
            nx = m.get("nextAiringEpisode") or None
            if nx and nx.get("airingAt") and nx.get("episode"):
                nx = {"episode": int(nx["episode"]),
                      "date": datetime.fromtimestamp(nx["airingAt"], JST).date().isoformat()}
            else:
                nx = None
            out.setdefault(int(mid), []).append((date.fromisoformat(d), m.get("status"), nx))
    except sqlite3.Error:
        pass
    finally:
        db.close()
    return out


def airing_on(airing: dict, sid: int, day: date) -> dict:
    """The show's AniList status and next episode as of `day` (the latest snapshot taken by then; nothing before the
    first snapshot: a halt or a date is news only from when it is seen)."""
    snaps = [a for a in airing.get(sid) or [] if a[0] <= day]
    if not snaps:
        return {}
    _, status, nx = snaps[-1]
    return {**({"hiatus": True} if status == "HIATUS" else {}), **({"next_airing": nx} if nx else {})}


def schedule_on(sched: dict, sid: int, day: date) -> list[str]:
    """The show's public schedule as of `day`: the latest snapshot taken by then, else the earliest (the dates of a
    season's episodes are published at its start; the replay's collector copy only has a late snapshot)."""
    snaps = sched.get(sid) or []
    before = [s for s in snaps if s[0] <= day]
    return (before[-1] if before else snaps[0])[1] if snaps else []


PRIOR_LAG = 7   # days: the pre-air estimate this long before, for the show's own growth (valuation `preair_trend`)


def prior_prev(s: E.Show, day: date, val: dict, aud_dated: dict) -> dict:
    """{"prior_prev": the point-in-time pre-air estimate PRIOR_LAG days earlier} while the show has not premiered and
    its AniList data reaches back that far; else nothing."""
    back = day - timedelta(days=PRIOR_LAG)
    if day >= s.first_air - timedelta(days=1) or not s.days or back < s.days[0]:
        return {}
    p = E.preair_pit(s, back, val, aud_dated)
    return {"prior_prev": p} if p.get("us") else {}


def top_decile(shows: list[E.Show], s: E.Show) -> bool:
    """In the top decile of popularity among the shows premiering within half a year of it."""
    peers = [x.popularity_end for x in shows if x.first_air and abs((x.first_air - s.first_air).days) <= 182]
    return bool(peers) and sum(1 for v in peers if v < s.popularity_end) / len(peers) >= 0.9


PLATFORM_DAYS = 365   # the platform size is the mean over the trailing year (four quarters)


def platform_size(shows: list[E.Show], markets: dict, day: date, p: dict) -> tuple[dict[str, float], int]:
    """Each market's platform size on `day` (docs/design/series.md §6): the mean over the trailing year (the days
    before `day` that have data, up to 365) of the day's total attention of every tracked work airing that day,
    listed or not (airing: an episode in the last 14 days, so the finale's week counts; adult titles and promos
    aside). Returns ({market: size}, the number of days averaged).

    A market with a fixed size (`revenue.platform_fixed_cn` / `_jp`) uses it instead: the trailing total depends on
    which works the collector covers, and its cn/jp series reach back only for the works collected now, so it rose
    12-18x over the 2026 replay window, a coverage artifact that would have cut every show's revenue month by month.
    Bangumi as a whole is flat (all anime subjects' new marks, 2025-06..2026-11: 13-26k a day, no trend); Annict has
    no site-wide figure (its growth, about 1.2x a year, then lifts jp revenue slowly). The scale set at the switch
    carries the units."""
    rv = p["revenue"]
    lo = day - timedelta(days=PLATFORM_DAYS)
    tot = airing_totals(shows, markets, rv, lo, day)
    first = min((s.days[0] for s in shows if s.days), default=day)
    days = [d for d in sorted(tot) if d >= max(lo, first)]
    # us: the days the rebuilt history covers come from it (every airing work, not only the ones this collector met)
    hist = platform_history(rv)
    us = {d: tot[d]["us"] for d in days}
    us.update({d: v for d, v in hist.items() if lo <= d < day})
    fixed = {m: float(rv[f"platform_fixed_{m}"]) for m in ("cn", "jp") if rv.get(f"platform_fixed_{m}")}
    n = len(us)
    if not n:
        return {m: fixed.get(m, 0.0) for m in E.MARKETS}, 0
    out = {m: fixed.get(m, sum(tot[d][m] for d in days) / len(days) if days else 0.0) for m in ("cn", "jp")}
    return {"us": sum(us.values()) / n, **out}, n


def airing_totals(shows: list[E.Show], markets: dict, rv: dict, lo: date, day: date) -> dict[date, dict[str, float]]:
    """Each day in [lo, day) -> each market's total attention of the works airing that day (an episode in the last 14
    days, so the finale's week counts; adult titles and promos aside): the platform's daily size."""
    tot: dict[date, dict[str, float]] = {}
    for s in shows:
        if s.adult or s.promo or not s.days:
            continue
        d = max(lo, s.days[0])
        while d < day:
            i = (d - s.days[0]).days
            if i >= len(s.days):
                break
            back = s.aired[i - 14] if i >= 14 else 0
            if s.aired[i] > back:
                a = E.market_att(s, d, rv, markets)
                t = tot.setdefault(d, {m: 0.0 for m in E.MARKETS})
                for m in E.MARKETS:
                    t[m] += a[m]
            d += timedelta(days=1)
    return tot


def platform_history(rv: dict) -> dict[date, float]:
    """`revenue.platform_history`: the us platform's daily size rebuilt from AniList's trends of every work the collector
    tracks (sim/platform_history.py; a file in the derived directory), for the days before the live collector covered
    them all: the server's collector started on 2026-11-05 and backfilled only the works it met, so its earlier days
    miss every show that had finished by then. Configured but missing is an error (the platform, and so every show's
    us revenue, would be wrong), not an empty history."""
    name = rv.get("platform_history")
    if not name:
        return {}
    f = Path(os.environ.get("ANISTOCK_DERIVED_DIR", ROOT / "data" / "derived")) / name
    if not f.exists():
        raise FileNotFoundError(f"{f} missing: revenue.platform_history needs it (sim/platform_history.py)")
    return {date.fromisoformat(d): float(v) for d, v in json.loads(f.read_text(encoding="utf-8"))["us"].items()}


# A show still airing whose cn/jp series has had no data for this long counts as unmapped in that market
# (no revenue there) instead of having it inferred from the us market: see `airing_sources`.
STALE_DAYS = 28
STALE_EPISODES = 2


def airing_sources(s: E.Show, day: date, markets: dict, mapped: dict[str, bool], aired: int | None = None) -> dict[str, bool]:
    """The cn/jp `sources` flags on `day`, given the point-in-time mapping. A show still airing (a long-runner,
    or fewer episodes aired than announced), premiered at least STALE_DAYS ago, with no value in a market's
    rebuilt series for the last STALE_DAYS days and at least STALE_EPISODES episodes aired since its last value,
    is unmapped there: the rebuild reads Bangumi/Annict episode pages, which for many long-runners stop updating
    (Chiikawa's end in May 2025), and a missing market is otherwise filled from the us level through a relation
    fitted on new shows' first weeks, which priced some long-runners several times over. A break in the
    broadcast is not a dead source (BLEACH's month without an episode in Oct 2026 switched its cn market off and
    back on, a 3x fall and rise): with no new episode there is nothing to report; nor is a source that never
    reported (a show matched late on Annict starts with the inferred level, then its data: switching it off in
    between moved the value twice). A finished show keeps its source: after the finale a missing market is the
    long tail's (docs/design/series.md §4). `aired`: the episode count on `day` (default: AniList's)."""
    out = {m: bool(mapped.get(m)) for m in ("cn", "jp")}
    if (day - s.first_air).days < STALE_DAYS:
        return out
    if aired is None:
        aired = E.at(s, s.aired, day, 0)
    if not s.long_runner and s.episodes and aired >= s.episodes:
        return out
    rows = markets.get(s.id, {})
    for m in ("cn", "jp"):
        if not out[m] or any(m in rows.get((day - timedelta(days=k)).isoformat(), {}) for k in range(STALE_DAYS)):
            continue
        last = max((d for d, r in rows.items() if m in r and d <= day.isoformat()), default=None)
        if last is None:    # nothing stopped: a new show whose data has not started is the inferred level's case
            continue
        if aired - E.at(s, s.aired, date.fromisoformat(last), 0) >= STALE_EPISODES:
            out[m] = False
    return out


APPRAISAL_WEEKS = (26, 18)      # the appraisal window: weeks 26 to 18 before the sequel's premiere (docs/design/series.md §13)
APPRAISAL_DAYS = 56             # its length (8 weeks)
APPRAISAL_RECENT_DAYS = 90      # a work's decline after its finale: no window starts within this of a finale
APPRAISAL_TAIL_DAYS = 120       # still declining: the in-season level times the long tail this long after the finale
APPRAISAL_FROM_DAYS = 60        # a work premiered within this of D joins a company as a new season would, not appraised


def load_member_records(db_path: Path) -> dict[int, dict[str, int]] | None:
    """The series members' Annict records per day (collect.py `count_member_records`): {Annict id: {day: records}},
    an entry for every work counted (none counted yet: an empty dict); None when the collector has no such table."""
    db = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    try:
        tables = {r[0] for r in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        if "member_records_done" not in tables:
            return None
        out: dict[int, dict[str, int]] = {int(r[0]): {} for r in db.execute("SELECT annict_id FROM member_records_done")}
        for aid, d, n in db.execute("SELECT annict_id, day, records FROM member_records"):
            out.setdefault(int(aid), {})[d] = n
        return out
    finally:
        db.close()


def member_records_by_show(db_path: Path) -> dict[int, dict[str, int]]:
    """`load_member_records` by AniList id (the effective Annict match), for the works counted."""
    rec = load_member_records(db_path) or {}
    return {int(k): rec[r["annict"]] for k, r in E.effective_matches(db_path).items() if r.get("annict") in rec}


def appraisal_window(s_days0: date, fin: date, day: date, others: list[tuple[date, date | None]]) -> tuple[date, date, bool] | None:
    """The appraisal window [lo, hi) of an earlier work that finished on `fin` (docs/design/series.md §13): the latest
    APPRAISAL_DAYS days, up to D, clear of every other work of the series from its run-up (APPRAISAL_WEEKS[1] weeks
    before its premiere; the sequel's announcement and promotion lift the earlier works) to APPRAISAL_RECENT_DAYS after
    its finale (`others`: premiere, finale or None while airing or unaired), and starting APPRAISAL_RECENT_DAYS after
    the work's own finale week and within its data (`s_days0`). With the series' newest work the only other one, this
    is weeks 26-18 before that work's premiere. None: no window after the work's own decline (it finished recently).
    No clean window within the data: the latest window there, and False."""
    span = timedelta(days=APPRAISAL_DAYS)
    first = max(s_days0, fin + timedelta(days=7 + APPRAISAL_RECENT_DAYS))
    if first + span > day:
        return None
    far = date.max - timedelta(days=APPRAISAL_RECENT_DAYS + 1)
    busy = [(p - timedelta(weeks=APPRAISAL_WEEKS[1]), (f or far) + timedelta(days=APPRAISAL_RECENT_DAYS)) for p, f in others]
    for hi in sorted({day} | {a for a, _ in busy if first + span <= a <= day}, reverse=True):
        lo = hi - span
        if all(b <= lo or a >= hi for a, b in busy):
            return lo, hi, True
    return day - span, day, False


def appraisal_att(s: E.Show, day: date, others: list[tuple[date, date | None]], rv: dict, markets: dict, fp: dict,
                  mapped: dict, records: dict[str, int] | None, cn_listed: bool = True) -> dict | None:
    """A finished earlier work's normal daily attention per market, the base of its public appraisal when it joins a
    listed company by an asset injection (docs/design/series.md §13): the mean over `appraisal_window` (clear of the
    other works' run-ups and runs), each market as the game reads it (cn/jp only where the work is matched: `mapped`);
    Japan from the work's Annict records per day (`records`; the rebuilt series has no post-finale days). A work still
    in its decline after the finale (no window yet), or with no clear window and its run in the data: the in-season
    mean times the fitted long tail APPRAISAL_TAIL_DAYS after the finale's week; with no clear window and no in-season
    data: the latest window anyway ("window_mixed"). None when the data is not all there (the trends or the records not
    read back yet, a Bangumi match whose episode list the collector has not read yet: `cn_listed`): the company waits.
    A subject with no main episodes (a film) has no viewer series: China 0."""
    from engine import valuation as V
    dates = air_dates(s, day)
    if not dates or s.long_runner or (s.episodes and len(dates) < s.episodes):
        return None
    fin = date.fromisoformat(dates[-1])
    if fin + timedelta(days=7) > day or not s.days:
        return None
    if mapped.get("cn") and not cn_listed:
        return None     # matched on Bangumi, its episode list not read yet (the collection after the match)
    w = appraisal_window(s.days[0], fin, day, others)
    season = [d for d in (s.first_air + timedelta(days=k) for k in range((fin - s.first_air).days + 7))
              if s.days[0] <= d <= s.days[-1]]
    if w is None or (not w[2] and season):
        # still declining after its finale, or no window clear of the other works (a sequel's run-up began before the
        # decline ended): the in-season level times the long tail
        days = season
        if not days:
            return None
        out = {}
        for m in E.MARKETS:
            tl = V.tail_of(fp, m)
            if tl is None:
                return None
            on = m == "us" or bool(mapped.get(m))
            mean = sum(E.market_att(s, d, rv, markets)[m] for d in days) / len(days) if on else 0.0
            out[m] = round(mean * V.tail_mean(tl, APPRAISAL_TAIL_DAYS, 1.0), 4)
        return {**out, "basis": "season", "from": days[0].isoformat(), "to": days[-1].isoformat(),
                "tail_days": APPRAISAL_TAIL_DAYS}
    lo, hi, clean = w
    jp_on = bool(mapped.get("jp"))
    if jp_on and records is None:
        return None
    days = [lo + timedelta(days=k) for k in range((hi - lo).days)]
    out = {"us": 0.0, "cn": 0.0, "jp": 0.0}
    for d in days:
        a = E.market_att(s, d, rv, markets)
        out["us"] += a["us"]
        out["cn"] += a["cn"] if mapped.get("cn") else 0.0
        if jp_on:
            out["jp"] += records.get(d.isoformat(), 0) * E.score_mult(E.at(s, s.score, d, None), rv)
    return {**{m: round(v / len(days), 4) for m, v in out.items()}, "basis": "window" if clean else "window_mixed",
            "from": lo.isoformat(), "to": (hi - timedelta(days=1)).isoformat()}


def series_titles(shows: list[E.Show], keys: dict[int, int], cn_titles: dict[int, str], ex: dict) -> dict[int, str]:
    """Each series of more than one work: its name (docs/design/series.md §13.7), the name a company of several works
    goes by. sim/series.json `names` (by any work of the series), else the Chinese title (Bangumi), else the native
    one, of its first TV, TV_SHORT or ONA work (films, OVAs and specials are side works), else of its first work."""
    by: dict[int, list[E.Show]] = {}
    for s in shows:
        by.setdefault(keys[s.id], []).append(s)
    names = ex.get("names", {})
    out = {}
    for k, ws in by.items():
        if len(ws) < 2:
            continue
        named = next((names[s.id] for s in sorted(ws, key=lambda s: s.id) if s.id in names), None)
        if named:
            out[k] = named
            continue
        main = [s for s in ws if s.fmt in ("TV", "TV_SHORT", "ONA")] or ws
        first = min(main, key=lambda s: (s.first_air, s.id))
        out[k] = cn_titles.get(first.id) or first.title
    return out


def build(shows: list[E.Show], markets: dict, aud: dict, cn_titles: dict[int, str], day: date, p: dict,
          window_start: date, aud_dated: dict | None = None, sources: dict | None = None,
          relations: dict | None = None, schedules: dict | None = None, airing: dict | None = None,
          heat: dict | None = None, records: dict[int, dict[str, int]] | None = None,
          overrides: dict[int, dict] | None = None) -> dict:
    """The inputs dict for `day` from already-loaded model objects (no I/O). records: the series members' Annict
    records per day by AniList id, for the works counted (`load_member_records`)."""
    rv, val = p["revenue"], p["valuation"]
    aud_dated = aud_dated or {}
    sources = sources or {}
    relations = relations or {}
    schedules = schedules or {}
    airing = airing or {}
    from engine import valuation as V
    lags = V.filter_params(p).get("sequel", {}).get("lag_days", [])
    from engine.series import load_exceptions, series_keys
    known = {i: [e for d, e in snaps if d <= day] for i, snaps in relations.items()}
    keys = series_keys({i: v[-1] for i, v in known.items() if v}, [s.id for s in shows], load_exceptions(),
                       {s.id: s.fmt for s in shows})
    records = records or {}
    fp = V.filter_params(p)
    runs: dict[int, list[tuple[int, date, date | None]]] = {}    # series -> each work's premiere and finale
    for s in shows:
        ad = air_dates(s, day)
        done = not s.long_runner and bool(s.episodes) and len(ad) >= s.episodes
        runs.setdefault(keys[s.id], []).append((s.id, s.first_air, date.fromisoformat(ad[-1]) if done else None))
    out = []
    for s in shows:
        appraisal = None
        if len(runs[keys[s.id]]) > 1 and s.first_air < day - timedelta(days=APPRAISAL_FROM_DAYS):
            others = [(f, e) for i, f, e in runs[keys[s.id]] if i != s.id]
            appraisal = appraisal_att(s, day, others, rv, markets, fp, sources.get(s.id, {"cn": False, "jp": False}),
                                      records.get(s.id), any(d <= day for d, _ in schedules.get(s.id, [])))
        att = E.market_att(s, day, rv, markets)
        sched = schedule_on(schedules, s.id, day)
        aired = batch_aired(sched, day, E.at(s, s.aired, day, 0))[0]
        out.append({
            "id": s.id,
            "title_ja": s.title,
            "title_en": s.title_en,
            "studio": s.studio,
            "first_air": s.first_air.isoformat(),
            "episodes": s.episodes,
            "episodes_known": s.episodes_known,
            "long_runner": s.long_runner,
            "popularity_end": s.popularity_end,
            "aired": aired,
            "has_real_data": E.has_real_data(s, day, val),
            "att": {m: att[m] for m in E.MARKETS},
            "preair": {m: v for m, v in E.preair_att(s, day, val, aud).items()},
            "prior": E.preair_pit(s, day, val, aud_dated),
            **prior_prev(s, day, val, aud_dated),
            "observed": E.observed_markets(s, day, markets),
            "sources": {"us": True, **airing_sources(s, day, markets, sources.get(s.id, {"cn": False, "jp": False}), aired)},
            **({"adult": True} if s.adult else {}),
            **({"promo": True} if s.promo else {}),
            "air_dates": air_dates(s, day, sched),
            "score": E.at(s, s.score, day, None),
            "format": s.fmt,
            "source": s.source,
            "genres": s.genres,
            **apply_sequel_override(series_facts(s.id, relations, day, lags), s.id, (overrides or {}).get(s.id), day,
                                    lags),
            "top_pop": top_decile(shows, s),
            "popularity": E.at(s, s.popularity, day, None),
            "schedule": sched,
            **airing_on(airing, s.id, day),
            "series": keys[s.id],
            **({"appraisal_att": appraisal} if appraisal else {}),
            "expected": {m: v for m, v in E.expected_att(s, day, rv, markets, val, aud).items()},
        })
    plat, n = platform_size(shows, markets, day, p)
    return {
        "day": day.isoformat(),
        "window_start": window_start.isoformat(),
        "cn_titles": {str(k): v for k, v in cn_titles.items()},
        "series_titles": {str(k): v for k, v in series_titles(shows, keys, cn_titles, load_exceptions()).items()},
        "platform": plat,
        "platform_days": n,
        "shows": out,
        **({"heat": heat} if heat else {}),
    }


def load_heat(day: date, path: Path | None = None) -> dict[str, dict]:
    """The published heat index of `day` per show (engine/heat_index.py), {} if it has not been computed."""
    from engine.heat_index import OUT
    path = path or OUT
    if not path.exists():
        return {}
    db = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    try:
        rows = db.execute("SELECT work_id, idx, us, cn, jp, flags, version FROM heat_index WHERE day=?",
                          (day.isoformat(),)).fetchall()
    except sqlite3.OperationalError:   # no table yet
        return {}
    return {str(w): {"idx": idx, "us": us, "cn": cn, "jp": jp, "flags": json.loads(flags), "version": v}
            for w, idx, us, cn, jp, flags, v in rows}


def window_start_of(cfg) -> date:
    """The game's window start (game state), else the parameters' window start."""
    if cfg.game_db.exists():
        db = sqlite3.connect(f"file:{cfg.game_db}?mode=ro", uri=True)
        try:
            row = db.execute("SELECT value FROM state WHERE key='window_start'").fetchone()
        except sqlite3.Error:
            row = None
        finally:
            db.close()
        if row:
            return date.fromisoformat(row[0])
    return date.fromisoformat(cfg.params["window"]["start"])


def archive_path() -> Path:
    """The Bangumi archive's weekly counts (data/derived/heat_backfill.db; sim/reconstruct.py reads the same file)."""
    return Path(os.environ.get("ANISTOCK_DERIVED_DIR", ROOT / "data" / "derived")) / "heat_backfill.db"


def source_dates(db_path: Path, archive: Path | None = None) -> dict[int, dict[str, date | None]]:
    """AniList id -> {"cn", "jp"}: the first day the collector had data for the show's mapped Bangumi subject /
    Annict work (its `works` row or its first snapshot, whichever is earlier), None if it never did or the show
    is not mapped. An Annict work matched through its MAL id also needs the AniList media's MAL id (its first
    snapshot), since the match is only possible from then on.

    A show the collector already had on its first day counts from date.min: the collector cannot tell when
    its source came online before it started (a collector database created mid-season would otherwise switch
    every show's cn/jp off for the whole history before that day; the rebuilt series cover those days).

    A Bangumi subject in the archive (`archive`, default `archive_path()`) counts from the day after its first weekly
    dump if that is earlier (the viewer series reads the dumps before the collector's first snapshot,
    engine/cn_views.py load_stocks): a finished show the collector meets late has had cn data since the dumps
    began. Without the archive file, a show matched through the archive ("bgm_via" "archive") is an error, not a
    late start."""
    matches = E.effective_matches(db_path)
    archive = archive or archive_path()
    dumped: dict[str, date] = {}
    if archive.exists():
        a = sqlite3.connect(f"file:{archive}?mode=ro", uri=True)
        try:
            tables = {r[0] for r in a.execute("SELECT name FROM sqlite_master WHERE type='table'")}
            for t in ("bgm_weekly", "bgm_anime_weekly"):
                if t in tables:
                    for sid, at in a.execute(f"SELECT subject_id, MIN(at) FROM {t} GROUP BY 1"):
                        d = date.fromisoformat(at[:10]) + timedelta(days=1)
                        dumped[str(sid)] = min(dumped.get(str(sid), d), d)
        finally:
            a.close()
    elif any(r.get("bgm_via") == "archive" for r in matches.values()):
        raise FileNotFoundError(f"{archive} missing: shows matched through the Bangumi archive need its dates "
                                "(docs/ops/switch-cn-views.md copies it to the server)")
    seen: dict[tuple[str, str], date] = {}
    if db_path.exists():
        db = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
        try:
            tables = {r[0] for r in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
            rows = []
            if "works" in tables:
                rows += db.execute("SELECT source, external_id, MIN(first_seen) FROM works GROUP BY 1, 2").fetchall()
            if "snapshots" in tables:
                rows += db.execute("SELECT source, external_id, MIN(day) FROM snapshots GROUP BY 1, 2").fetchall()
            days = [r[2] for r in rows if r[2]]
            if "runs" in tables:
                first_run = db.execute("SELECT MIN(started_at) FROM runs").fetchone()[0]
                if first_run:
                    days.append(first_run)
        finally:
            db.close()
        born = min((date.fromisoformat(d[:10]) for d in days), default=None)
        for src, ext, d in rows:
            if d:
                k, v = (src, str(ext)), date.fromisoformat(d[:10])
                if born is not None and v <= born:
                    v = date.min
                seen[k] = min(seen.get(k, v), v)
    out = {}
    for aid, r in matches.items():
        cn = seen.get(("bgm", str(r["bgm"]))) if r.get("bgm") else None
        if r.get("bgm") and str(r["bgm"]) in dumped:
            cn = min(cn, dumped[str(r["bgm"])]) if cn is not None else dumped[str(r["bgm"])]
        jp = seen.get(("annict", str(r["annict"]))) if r.get("annict") else None
        if jp is not None and r.get("annict_via") == "mal_id":
            mal = seen.get(("anilist", str(aid)))
            jp = max(jp, mal) if mal else None
        out[int(aid)] = {"cn": cn, "jp": jp}
    return out


def sources_on(dates: dict[int, dict[str, date | None]], d: date) -> dict[int, dict[str, bool]]:
    """The `sources` flags on day D from `source_dates`."""
    return {i: {m: v is not None and v <= d for m, v in x.items()} for i, x in dates.items()}


def daily_inputs(day: str, cfg=None, window_start: date | None = None) -> dict:
    """Load the model's data and build the inputs for `day` (what the CLI writes)."""
    if cfg is None:
        from server.config import load
        cfg = load()
    d = date.fromisoformat(day)
    start = window_start or window_start_of(cfg)
    shows, _ = E.load_shows(cfg.collector_db, cfg.params, start, d,
                            upcoming_days=cfg.game["ipo_subscription_days"] + 1)
    return build(shows, load_markets(), E.load_audiences(cfg.collector_db), E.load_cn_titles(cfg.collector_db),
                 d, cfg.params, start, E.load_audiences_dated(cfg.collector_db), sources_on(source_dates(cfg.collector_db), d),
                 load_relations(cfg.collector_db), load_schedules(cfg.collector_db), load_airing(cfg.collector_db),
                 load_heat(d), member_records_by_show(cfg.collector_db), load_sequel_overrides())


def dumps(inputs: dict) -> str:
    return json.dumps(inputs, ensure_ascii=False, separators=(",", ":"))


def write(inputs: dict, path: Path | None = None) -> Path:
    path = path or inputs_dir() / f"{inputs['day']}.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(dumps(inputs), encoding="utf-8")
    tmp.replace(path)
    return path


# ---------------------------------------------------------------- reading (used by the daily close)

@dataclass(frozen=True)
class ShowInputs:
    id: int
    title_ja: str
    title_en: str | None
    studio: str | None
    first_air: date
    episodes: int | None
    episodes_known: bool
    long_runner: bool
    popularity_end: int
    aired: int
    has_real_data: bool
    att: dict
    expected: dict
    preair: dict | None = None             # since v0.3.0
    air_dates: list = field(default_factory=list)
    adult: bool = False                    # since v0.3.1
    promo: bool = False                    # since v0.3.2
    prior: dict | None = None              # since v0.3.2
    observed: dict | None = None           # since v0.3.2
    sources: dict | None = None            # since v0.3.2
    score: float | None = None             # phase 1
    format: str | None = None              # phase 1
    source: str | None = None              # phase 1
    prequel: bool = False                  # phase 1
    top_pop: bool = False                  # phase 1
    sequel: dict | None = None             # phase 1
    series: int | None = None              # phase 1
    popularity: float | None = None        # phase 1
    relations_known: bool = False          # phase 1
    sequel_cancelled: str | None = None    # v0.8.4: an official "no sequel" (its source; docs/design/series.md §5.4)
    schedule: list = field(default_factory=list)   # phase 1
    hiatus: bool = False                   # docs/design/corporate.md
    next_airing: dict | None = None       # docs/design/corporate.md
    genres: list = field(default_factory=list)     # docs/design/funds.md (the theme indices; the close reads it)
    prior_prev: dict | None = None         # the pre-air estimate 7 days earlier (valuation `preair_trend`)


@dataclass(frozen=True)
class Inputs:
    day: str
    shows: dict[int, ShowInputs]     # model order
    cn_titles: dict[int, str]
    expected: dict[int, dict]        # id -> ShowInputs.expected, for the expected-share denominator


def parse(raw: dict) -> Inputs:
    shows = {}
    for r in raw["shows"]:
        shows[r["id"]] = ShowInputs(**{**r, "first_air": date.fromisoformat(r["first_air"])})
    return Inputs(day=raw["day"], shows=shows, cn_titles={int(k): v for k, v in raw["cn_titles"].items()},
                  expected={i: s.expected for i, s in shows.items()})


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--day", required=True, help="the day being closed, YYYY-MM-DD")
    ap.add_argument("--out", type=Path, help="output path (default: data/derived/inputs/<day>.json)")
    ap.add_argument("--window-start", help="override the game's window start (default: game DB state)")
    ap.add_argument("--force", action="store_true", help="with --from: rewrite files that exist")
    ap.add_argument("--from", dest="start",
                    help="also write every day from this one up to --day into the default directory, skipping "
                         "files that exist (history for the server's valuation bootstrap at go-live)")
    args = ap.parse_args()
    ws = date.fromisoformat(args.window_start) if args.window_start else None
    if args.start:
        d, end = date.fromisoformat(args.start), date.fromisoformat(args.day)
        while d <= end:
            if args.force or not (inputs_dir() / f"{d.isoformat()}.json").exists():
                inputs = daily_inputs(d.isoformat(), window_start=ws)
                print(f"{write(inputs, None)}: {len(inputs['shows'])} shows")
            d += timedelta(days=1)
        return
    inputs = daily_inputs(args.day, window_start=ws)
    path = write(inputs, args.out)
    print(f"{path}: {len(inputs['shows'])} shows")


if __name__ == "__main__":
    main()
