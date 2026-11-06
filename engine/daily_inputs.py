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
    platform      {"us","cn","jp"}  each market's platform size (docs/v2-series.md §6): the mean, over the trailing
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
        prequel         itself a sequel: the relations known on D have a PREQUEL TV/ONA/film (phase 1)
        top_pop         top decile of popularity among the shows premiering within half a year of it (phase 1)
        sequel          an announced sequel not yet aired, from the relations known on D, else null (phase 1;
                        engine/daily_inputs.py `series_facts`): {"id", "announced" (the first day the relation was
                        seen), "premieres" [days, equally likely], "exact" (an announced day)}
        popularity      AniList list entries on D (the tail prior's popularity at the finale) (phase 1)
        relations_known the collector has the show's AniList relations as of D; without them "no sequel announced
                        yet" is not information and the sequel option is not updated as time passes (phase 1)
        schedule        ["YYYY-MM-DD", ...] every main episode's air date on Bangumi (the public schedule); a show
                        whose schedule has an episode after the last one AniList recorded is not finished (phase 1);
                        the valuation dates the episodes still to air by it (docs/v2-corporate.md §2.2)
        hiatus          AniList status HIATUS on D (production halted; omitted when false) (docs/v2-corporate.md §2.3)
        next_airing     AniList nextAiringEpisode on D: {"episode", "date" (Japanese broadcast date)}; omitted
                        without one (docs/v2-corporate.md §2.2)
        series          the series key (engine/series.py: SEQUEL/PREQUEL among TV/ONA/films, plus sim/series.json;
                        the smallest AniList id in the series); a new season of a listed series joins its company
                        instead of listing (phase 1)
        expected        {"us","cn","jp"}  expected attention on D: `att` once the show has real data, else the
                        per-market max of `att` and the audience-based pre-air estimate (expected-share
                        denominator, IPO pricing, fair values)

Not in the file, because they come from game state or config rather than the model: the demand index
(from Σ att.us over *listed* companies and the game's `a_ref`), market weights, rates, prices.
"""

from __future__ import annotations

import argparse
import json
import os
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


def air_dates(s: E.Show, day: date) -> list[str]:
    """The day each episode aired so far (from the model's per-day episode count)."""
    out, prev = [], 0
    for d, a in zip(s.days, s.aired):
        if d > day:
            break
        if a > prev:
            out.extend([d.isoformat()] * (a - prev))
        prev = a
    return out


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


def series_facts(sid: int, rel: dict, day: date, lags: list[int]) -> dict:
    """What the relations known on `day` say (docs/v2-series.md §5): whether the show is itself a sequel (a PREQUEL
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
        if prec == "none":
            prem = [first + timedelta(days=lag) for lag in lags]
        else:
            prem = [max(start, day + timedelta(days=1))]
        cands.append((min(prem), {"id": n["id"], "announced": first.isoformat(), "premieres": [d.isoformat() for d in prem],
                                  "exact": prec == "day"}))
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
    """AniList's status and next episode per daily media snapshot (docs/v2-corporate.md §2.2–2.3):
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


def top_decile(shows: list[E.Show], s: E.Show) -> bool:
    """In the top decile of popularity among the shows premiering within half a year of it."""
    peers = [x.popularity_end for x in shows if x.first_air and abs((x.first_air - s.first_air).days) <= 182]
    return bool(peers) and sum(1 for v in peers if v < s.popularity_end) / len(peers) >= 0.9


PLATFORM_DAYS = 365   # the platform size is the mean over the trailing year (four quarters)


def platform_size(shows: list[E.Show], markets: dict, day: date, p: dict) -> tuple[dict[str, float], int]:
    """Each market's platform size on `day` (docs/v2-series.md §6): the mean over the trailing year (the days
    before `day` that have data, up to 365) of the day's total attention of every tracked work airing that day,
    listed or not (airing: an episode in the last 14 days, so the finale's week counts; adult titles and promos
    aside). Returns ({market: size}, the number of days averaged)."""
    rv = p["revenue"]
    lo = day - timedelta(days=PLATFORM_DAYS)
    tot: dict[date, dict[str, float]] = {}
    first = min((s.days[0] for s in shows if s.days), default=day)
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
    days = [d for d in sorted(tot) if d >= max(lo, first)]
    n = len(days)
    if not n:
        return {m: 0.0 for m in E.MARKETS}, 0
    return {m: sum(tot[d][m] for d in days) / n for m in E.MARKETS}, n


def build(shows: list[E.Show], markets: dict, aud: dict, cn_titles: dict[int, str], day: date, p: dict,
          window_start: date, aud_dated: dict | None = None, sources: dict | None = None,
          relations: dict | None = None, schedules: dict | None = None, airing: dict | None = None) -> dict:
    """The inputs dict for `day` from already-loaded model objects (no I/O)."""
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
    keys = series_keys({i: v[-1] for i, v in known.items() if v}, [s.id for s in shows], load_exceptions())
    out = []
    for s in shows:
        att = E.market_att(s, day, rv, markets)
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
            "aired": E.at(s, s.aired, day, 0),
            "has_real_data": E.has_real_data(s, day, val),
            "att": {m: att[m] for m in E.MARKETS},
            "preair": {m: v for m, v in E.preair_att(s, day, val, aud).items()},
            "prior": E.preair_pit(s, day, val, aud_dated),
            "observed": E.observed_markets(s, day, markets),
            "sources": {"us": True, **sources.get(s.id, {"cn": False, "jp": False})},
            **({"adult": True} if s.adult else {}),
            **({"promo": True} if s.promo else {}),
            "air_dates": air_dates(s, day),
            "score": E.at(s, s.score, day, None),
            "format": s.fmt,
            "source": s.source,
            **series_facts(s.id, relations, day, lags),
            "top_pop": top_decile(shows, s),
            "popularity": E.at(s, s.popularity, day, None),
            "schedule": schedule_on(schedules, s.id, day),
            **airing_on(airing, s.id, day),
            "series": keys[s.id],
            "expected": {m: v for m, v in E.expected_att(s, day, rv, markets, val, aud).items()},
        })
    plat, n = platform_size(shows, markets, day, p)
    return {
        "day": day.isoformat(),
        "window_start": window_start.isoformat(),
        "cn_titles": {str(k): v for k, v in cn_titles.items()},
        "platform": plat,
        "platform_days": n,
        "shows": out,
    }


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


def source_dates(db_path: Path) -> dict[int, dict[str, date | None]]:
    """AniList id -> {"cn", "jp"}: the first day the collector had data for the show's mapped Bangumi subject /
    Annict work (its `works` row or its first snapshot, whichever is earlier), None if it never did or the show
    is not mapped. An Annict work matched through its MAL id also needs the AniList media's MAL id (its first
    snapshot), since the match is only possible from then on.

    A show the collector already had on its first day counts from date.min: the collector cannot tell when
    its source came online before it started (a collector database created mid-season would otherwise switch
    every show's cn/jp off for the whole history before that day; the rebuilt series cover those days)."""
    matches = E.effective_matches(db_path)
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
                 load_relations(cfg.collector_db), load_schedules(cfg.collector_db), load_airing(cfg.collector_db))


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
    schedule: list = field(default_factory=list)   # phase 1
    hiatus: bool = False                   # docs/v2-corporate.md
    next_airing: dict | None = None       # docs/v2-corporate.md


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
