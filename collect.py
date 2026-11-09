# /// script
# requires-python = ">=3.10"
# dependencies = ["httpx[socks]>=0.27"]
# ///
"""AniStock data collector.

Daily snapshots of currently-airing anime from Bangumi, AniList and Annict,
stored as raw JSON in SQLite so metric formulas can change later.

Usage:
    uv run collect.py                      # all sources
    uv run collect.py --source bgm         # one source
    uv run collect.py --backfill           # also backfill AniList daily trends history
    uv run collect.py --stats              # print row counts and exit

Env:
    ANISTOCK_DB          sqlite path (default: data/anistock.db next to this file)
    ANNICT_TOKEN         Annict personal access token (read-only). Annict is skipped if unset.
    ANNICT_EXTRA_IDS     comma-separated Annict work IDs to always track (long-running shows)
    HTTPS_PROXY / NO_PROXY  standard proxy env vars (honoured by httpx)
"""

from __future__ import annotations

import argparse
import json
import logging
import os
import queue
import re
import sqlite3
import sys
import threading
import time
import unicodedata
from collections import deque
from collections.abc import Callable
from concurrent.futures import ThreadPoolExecutor, wait
from dataclasses import dataclass, field
from datetime import date, datetime, timedelta, timezone
from pathlib import Path
from zoneinfo import ZoneInfo

import httpx

TZ = ZoneInfo("Asia/Shanghai")  # "day" boundaries are defined in this zone
# developer/app (url): Bangumi asks for a developer id and app name, the others for a way to reach us (docs/design/data-terms.md)
USER_AGENT = "HuanCheng65/anistock (https://github.com/HuanCheng65/anistock)"
KEEP_TRACKING_DAYS = 180  # full snapshots (episode lists) this long after a work was last discovered; after that the
                          # work stays tracked for good in a lighter "tail" mode (docs/design/series.md §4.2: revenue in the
                          # long tail needs daily data for as long as the company is listed)
MIN_ANILIST_POPULARITY = 1000  # RELEASING filter, keeps out tiny shows
MIN_ANNICT_WATCHERS = 30

log = logging.getLogger("collect")


# ---------------------------------------------------------------- utilities

def load_dotenv(path: Path) -> None:
    """Minimal .env loader (KEY=VALUE lines, # comments). Real env vars win."""
    if not path.is_file():
        return
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, _, value = line.partition("=")
        value = value.split(" #", 1)[0].strip().strip('"').strip("'")
        os.environ.setdefault(key.strip(), value)


def now_local() -> datetime:
    return datetime.now(TZ)


def today() -> str:
    return now_local().date().isoformat()


def current_season(d: date) -> tuple[int, str]:
    """Returns (year, season) where season in winter/spring/summer/autumn (Jan/Apr/Jul/Oct)."""
    return d.year, ["winter", "spring", "summer", "autumn"][(d.month - 1) // 3]


def previous_season(year: int, season: str) -> tuple[int, str]:
    order = ["winter", "spring", "summer", "autumn"]
    i = order.index(season)
    return (year - 1, "autumn") if i == 0 else (year, order[i - 1])


def season_start(year: int, season: str) -> date:
    return date(year, {"winter": 1, "spring": 4, "summer": 7, "autumn": 10}[season], 1)


class Http:
    """httpx wrapper: retries, 429 Retry-After, and rate-limit header pacing."""

    def __init__(self, min_interval: float, remaining_header: str | None = None,
                 reset_header: str | None = None, headers: dict | None = None):
        self.client = httpx.Client(
            headers={"User-Agent": USER_AGENT, **(headers or {})},
            timeout=httpx.Timeout(30.0, connect=15.0),
            trust_env=True,
            follow_redirects=True,
        )
        self.min_interval = min_interval
        self.remaining_header = remaining_header
        self.reset_header = reset_header
        self._last = 0.0

    def request(self, method: str, url: str, **kw) -> httpx.Response:
        last = "no attempt made"
        for attempt in range(6):
            wait = self.min_interval - (time.monotonic() - self._last)
            if wait > 0:
                time.sleep(wait)
            self._last = time.monotonic()
            try:
                r = self.client.request(method, url, **kw)
            except httpx.TransportError as e:
                last = f"{type(e).__name__}: {e}"
                backoff = 2 ** attempt * 2
                log.warning("%s on %s, retry in %ss", last, url, backoff)
                time.sleep(backoff)
                continue
            if r.status_code == 429 or r.status_code >= 500:
                last = f"HTTP {r.status_code}: {r.text[:200]!r}"
            if r.status_code == 429:
                ra = r.headers.get("retry-after")
                backoff = int(ra) + 1 if ra and ra.isdigit() else 60
                log.warning("429 on %s, sleeping %ss", url, backoff)
                time.sleep(backoff)
                continue
            if r.status_code >= 500:
                backoff = 2 ** attempt * 2
                log.warning("%s on %s, retry in %ss", r.status_code, url, backoff)
                time.sleep(backoff)
                continue
            self._pace(r)
            return r
        raise RuntimeError(f"giving up on {method} {url} after 6 attempts, last error: {last}")

    def _pace(self, r: httpx.Response) -> None:
        if not self.remaining_header:
            return
        rem = r.headers.get(self.remaining_header)
        if rem is not None and rem.isdigit() and int(rem) <= 1:
            reset = r.headers.get(self.reset_header or "", "")
            secs = int(reset) if reset.isdigit() else 60
            # AniList sends an epoch timestamp, Annict sends seconds-remaining
            if secs > 10_000_000:
                secs = max(0, secs - int(time.time()))
            log.info("rate limit nearly exhausted, sleeping %ss", secs + 1)
            time.sleep(secs + 1)

    def graphql(self, url: str, query: str, variables: dict | None = None) -> dict:
        r = self.request("POST", url, json={"query": query, "variables": variables or {}})
        body = r.json()
        if r.status_code != 200 or body.get("errors"):
            raise RuntimeError(f"GraphQL error {r.status_code}: {json.dumps(body.get('errors'), ensure_ascii=False)[:500]}")
        return body["data"]


# ---------------------------------------------------------------- storage

SCHEMA = """
CREATE TABLE IF NOT EXISTS works (
    source          TEXT NOT NULL,          -- bgm | anilist | annict
    external_id     TEXT NOT NULL,
    title           TEXT,
    mal_id          TEXT,
    first_seen      TEXT NOT NULL,          -- day
    last_discovered TEXT NOT NULL,          -- last day it appeared in calendar/season listing
    PRIMARY KEY (source, external_id)
);
CREATE TABLE IF NOT EXISTS snapshots (
    source      TEXT NOT NULL,
    kind        TEXT NOT NULL,              -- bgm: subject|episodes  anilist: media  annict: work
    external_id TEXT NOT NULL,
    day         TEXT NOT NULL,              -- Asia/Shanghai date; one row per day, re-runs overwrite
    fetched_at  TEXT NOT NULL,              -- ISO timestamp with offset
    data        TEXT NOT NULL,              -- raw JSON
    PRIMARY KEY (source, kind, external_id, day)
);
CREATE TABLE IF NOT EXISTS anilist_trends (
    media_id    INTEGER NOT NULL,
    date_epoch  INTEGER NOT NULL,           -- as given by AniList
    date        TEXT NOT NULL,              -- derived, UTC date
    episode     INTEGER,
    data        TEXT NOT NULL,
    PRIMARY KEY (media_id, date_epoch)
);
CREATE TABLE IF NOT EXISTS annict_viewers (
    episode_id  INTEGER NOT NULL,           -- Annict episode
    user_id     INTEGER NOT NULL,           -- Annict user: de-duplication and surveillance only, never published
    first_at    TEXT NOT NULL,              -- the user's first record of the episode
    PRIMARY KEY (episode_id, user_id)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS annict_episode_sync (
    episode_id  INTEGER PRIMARY KEY,
    work_id     INTEGER NOT NULL,
    records     INTEGER NOT NULL,           -- the episode's recordsCount when its records were last read
    newest_at   TEXT                        -- createdAt of the newest record read
);
CREATE TABLE IF NOT EXISTS runs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    source      TEXT NOT NULL,
    started_at  TEXT NOT NULL,
    finished_at TEXT,
    status      TEXT,                       -- ok | failed | skipped
    detail      TEXT
);
"""


class Store:
    def __init__(self, path: Path):
        path.parent.mkdir(parents=True, exist_ok=True)
        self.db = sqlite3.connect(path)
        self.db.execute("PRAGMA journal_mode=WAL")
        self.db.executescript(SCHEMA)
        self.day = today()

    def register(self, source: str, external_id, title: str | None, mal_id=None) -> None:
        self.db.execute(
            """INSERT INTO works(source, external_id, title, mal_id, first_seen, last_discovered)
               VALUES (?,?,?,?,?,?)
               ON CONFLICT(source, external_id) DO UPDATE SET
                 title=coalesce(excluded.title, title),
                 mal_id=coalesce(excluded.mal_id, mal_id),
                 last_discovered=excluded.last_discovered""",
            (source, str(external_id), title, str(mal_id) if mal_id else None, self.day, self.day),
        )

    def tracked(self, source: str, tail: bool | None = False) -> list[str]:
        """Works to snapshot today. tail=False: discovered within KEEP_TRACKING_DAYS (full snapshots);
        tail=True: older ones (the long tail, lighter snapshots); tail=None: all."""
        cutoff = (date.fromisoformat(self.day) - timedelta(days=KEEP_TRACKING_DAYS)).isoformat()
        if tail is None:
            rows = self.db.execute("SELECT external_id FROM works WHERE source=?", (source,))
        else:
            op = "<" if tail else ">="
            rows = self.db.execute(
                f"SELECT external_id FROM works WHERE source=? AND last_discovered{op}?", (source, cutoff))
        return [r[0] for r in rows]

    def snapshot(self, source: str, kind: str, external_id, data) -> None:
        self.db.execute(
            "INSERT OR REPLACE INTO snapshots VALUES (?,?,?,?,?,?)",
            (source, kind, str(external_id), self.day, now_local().isoformat(timespec="seconds"),
             json.dumps(data, ensure_ascii=False, separators=(",", ":"))),
        )

    def trend(self, media_id: int, node: dict) -> None:
        d = datetime.fromtimestamp(node["date"], timezone.utc).date().isoformat()
        self.db.execute(
            "INSERT OR REPLACE INTO anilist_trends VALUES (?,?,?,?,?)",
            (media_id, node["date"], d, node.get("episode"),
             json.dumps(node, separators=(",", ":"))),
        )

    def latest_trend_epoch(self, media_id: int) -> int | None:
        r = self.db.execute("SELECT max(date_epoch) FROM anilist_trends WHERE media_id=?", (media_id,)).fetchone()
        return r[0]

    def run_start(self, source: str) -> int:
        # runs killed mid-way (reboot, Ctrl-C) never get a status; close them out
        stale = (now_local() - timedelta(hours=2)).isoformat(timespec="seconds")
        self.db.execute("UPDATE runs SET status='aborted' WHERE status IS NULL AND started_at < ?", (stale,))
        cur = self.db.execute("INSERT INTO runs(source, started_at) VALUES (?,?)",
                              (source, now_local().isoformat(timespec="seconds")))
        self.db.commit()
        return cur.lastrowid

    def run_end(self, run_id: int, status: str, detail: str = "") -> None:
        self.db.execute("UPDATE runs SET finished_at=?, status=?, detail=? WHERE id=?",
                        (now_local().isoformat(timespec="seconds"), status, detail[:2000], run_id))
        self.db.commit()

    def commit(self) -> None:
        self.db.commit()


# ---------------------------------------------------------------- Bangumi

BGM = "https://api.bgm.tv"


def manual_bgm_ids(path: Path) -> list[int]:
    """The Bangumi subjects sim/matches.json "manual" assigns (none when the file is missing or unreadable)."""
    try:
        manual = json.loads(path.read_text(encoding="utf-8")).get("manual") or {}
    except (OSError, ValueError):
        return []
    return sorted({int(v["bgm"]) for v in manual.values() if isinstance(v, dict) and v.get("bgm")})


def archive_bgm_ids(path: Path, shows: set[str]) -> list[int]:
    """The Bangumi subjects sim/matches.json "archive" assigns to the AniList shows in `shows` (the ones this collector
    has): an archive match for a show the collector never met costs requests and feeds nothing."""
    try:
        archive = json.loads(path.read_text(encoding="utf-8")).get("archive") or {}
    except (OSError, ValueError):
        return []
    return sorted({int(v["bgm"]) for k, v in archive.items() if k in shows and isinstance(v, dict) and v.get("bgm")})


def collect_bangumi(store: Store) -> str:
    http = Http(min_interval=0.4)
    cal = http.request("GET", f"{BGM}/calendar").json()
    discovered = 0
    for day in cal:
        for item in day.get("items", []):
            if item.get("type") != 2:  # anime only
                continue
            store.register("bgm", item["id"], item.get("name_cn") or item.get("name"))
            discovered += 1
    # airing shows Bangumi's calendar misses (long-runners, Netflix drops...), found by sim/match.py
    extra_file = Path(__file__).parent / "tracked_extra.json"
    if extra_file.is_file():
        for sid in json.loads(extra_file.read_text(encoding="utf-8")).get("bgm", []):
            store.register("bgm", sid, None)
            discovered += 1
    # hand matches (sim/matches.json "manual"): the matcher skips the shows they cover and their subject may be on
    # no calendar (a web release), so nothing else registers it: no snapshot, no cover, no Bangumi inputs
    for sid in manual_bgm_ids(Path(__file__).parent / "sim" / "matches.json"):
        store.register("bgm", sid, None)
        discovered += 1
    # the reviewed archive matches of shows this collector has (a finished show first met here, e.g. one listed later)
    for sid in archive_bgm_ids(Path(__file__).parent / "sim" / "matches.json", set(store.tracked("anilist", tail=None))):
        store.register("bgm", sid, None)
        discovered += 1
    store.commit()

    recent = store.tracked("bgm")
    tail = set(store.tracked("bgm", tail=True))
    ids = recent + sorted(tail)
    ok = failed = 0
    for sid in ids:
        try:
            r = http.request("GET", f"{BGM}/v0/subjects/{sid}")
            if r.status_code == 404:
                log.warning("bgm subject %s not found", sid)
                continue
            r.raise_for_status()
            if r.json().get("nsfw"):
                continue    # an adult subject (Bangumi's own flag): never listed, so not kept (excluded_media)
            store.snapshot("bgm", "subject", sid, r.json())
            if sid in tail:
                # long tail: the collection counts (the cn tail metric) are in the subject; episode comments have
                # stopped by then, so skip the two episode requests
                store.commit()
                ok += 1
                continue

            # main episodes only (type=0); long-runners have 1000+ eps, so take the latest 100
            first = http.request("GET", f"{BGM}/v0/episodes",
                                 params={"subject_id": sid, "type": 0, "limit": 1}).json()
            total = first.get("total", 0)
            offset = max(0, total - 100)
            eps = http.request("GET", f"{BGM}/v0/episodes",
                               params={"subject_id": sid, "type": 0, "limit": 100, "offset": offset}).json()
            slim = [{k: e.get(k) for k in ("id", "ep", "sort", "airdate", "comment", "name", "name_cn")}
                    for e in eps.get("data", [])]
            store.snapshot("bgm", "episodes", sid, {"total": total, "offset": offset, "data": slim})
            store.commit()
            ok += 1
        except Exception as e:  # keep going; one bad subject must not kill the day
            failed += 1
            log.warning("bgm subject %s failed: %s", sid, e)
    if ok == 0 and ids:
        raise RuntimeError(f"all {len(ids)} bangumi subjects failed")
    return f"discovered={discovered} tracked={len(recent)} tail={len(tail)} ok={ok} failed={failed}"


# ---------------------------------------------------------------- AniList

ANILIST = "https://graphql.anilist.co"

MEDIA_FIELDS = """
  id idMal format status episodes duration source countryOfOrigin
  season seasonYear startDate { year month day } endDate { year month day }
  title { native romaji english }
  synonyms
  studios(isMain: true) { nodes { id name } }
  averageScore meanScore popularity favourites trending
  nextAiringEpisode { episode airingAt }
  genres isAdult
  tags { name rank }
  stats { statusDistribution { status amount } scoreDistribution { score amount } }
"""

PAGE_QUERY = """
query ($page: Int, $season: MediaSeason, $year: Int, $status: MediaStatus, $popGt: Int, $ids: [Int],
       $formats: [MediaFormat]) {
  Page(page: $page, perPage: 50) {
    pageInfo { hasNextPage }
    media(type: ANIME, season: $season, seasonYear: $year, status: $status,
          popularity_greater: $popGt, id_in: $ids, format_in: $formats, sort: POPULARITY_DESC) {
      %s
      recent: trends(sort: DATE_DESC, perPage: 10) {
        nodes { date episode averageScore popularity inProgress trending releasing }
      }
    }
  }
}
""" % MEDIA_FIELDS

TRENDS_QUERY = """
query ($id: Int, $page: Int) {
  Media(id: $id) {
    trends(sort: DATE_DESC, page: $page, perPage: 50) {
      pageInfo { hasNextPage }
      nodes { date episode averageScore popularity inProgress trending releasing }
    }
  }
}
"""


def _anilist_pages(http: Http, variables: dict) -> list[dict]:
    out, page = [], 1
    while True:
        data = http.graphql(ANILIST, PAGE_QUERY, {**variables, "page": page})
        out.extend(data["Page"]["media"])
        if not data["Page"]["pageInfo"]["hasNextPage"]:
            return out
        page += 1


# AniList serves at most 25 nested trend nodes a page, and counts a request, not the media in it: one request
# asks for many media by alias, up to the query complexity limit (500; about 12 per media)
TRENDS_PER_PAGE = 25
TRENDS_BATCH = 35


def _trends_batch(pages: dict[int, int]) -> str:
    parts = [f"m{mid}: Media(id: {mid}) {{ trends(sort: DATE_DESC, page: {page}, perPage: {TRENDS_PER_PAGE}) "
             f"{{ pageInfo {{ hasNextPage }} nodes {{ date episode averageScore popularity inProgress trending releasing }} }} }}"
             for mid, page in pages.items()]
    return "query { " + " ".join(parts) + " }"


def _backfill_trends(http: Http, store: Store, media_ids: list[int], since: date) -> int:
    """Each media's daily trends back to `since`, newest first, many media per request. Resumable: a media
    whose trends were read back to `since` before (table trend_backfill) is skipped, so an interrupted
    backfill starts again where it stopped."""
    cutoff = int(datetime.combine(since, datetime.min.time(), timezone.utc).timestamp())
    store.db.execute("CREATE TABLE IF NOT EXISTS trend_backfill (media_id INTEGER PRIMARY KEY, since TEXT NOT NULL)")
    done = {r[0] for r in store.db.execute("SELECT media_id FROM trend_backfill WHERE since <= ?", (since.isoformat(),))}
    pages = {mid: 1 for mid in media_ids if mid not in done}
    log.info("anilist backfill: %d media to read back to %s (%d done before)", len(pages), since, len(media_ids) - len(pages))
    filled = 0
    while pages:
        batch = dict(list(pages.items())[:TRENDS_BATCH])
        data = http.graphql(ANILIST, _trends_batch(batch))
        for mid, page in batch.items():
            t = ((data.get(f"m{mid}") or {}).get("trends")) or {"nodes": [], "pageInfo": {"hasNextPage": False}}
            for node in t["nodes"]:
                store.trend(mid, node)
                filled += 1
            oldest = min((n["date"] for n in t["nodes"]), default=0)
            if not t["pageInfo"]["hasNextPage"] or oldest < cutoff:
                del pages[mid]
                store.db.execute("INSERT OR REPLACE INTO trend_backfill VALUES (?,?)", (mid, since.isoformat()))
            else:
                pages[mid] = page + 1
        store.commit()
    return filled


def collect_anilist_season(store: Store, spec: str) -> str:
    """One-off: register a past season's shows (e.g. 2026-summer) and backfill their trends.

    Finished shows drop out of the daily RELEASING listing, so a season that already ended
    is only reachable this way. Once registered they are tracked like any other work.
    """
    year_s, _, season = spec.partition("-")
    year = int(year_s)
    if season not in ("winter", "spring", "summer", "autumn"):
        raise ValueError(f"bad season {spec!r}, expected e.g. 2026-summer")
    http = Http(min_interval=2.1, remaining_header="x-ratelimit-remaining",
                reset_header="x-ratelimit-reset")
    media = _anilist_pages(http, {"season": season.upper().replace("AUTUMN", "FALL"), "year": year,
                                  "popGt": MIN_ANILIST_POPULARITY, "formats": ["TV", "TV_SHORT", "ONA"]})
    media = [m for m in media if not excluded_media(m)]
    for m in media:
        store.register("anilist", m["id"], m["title"].get("native") or m["title"].get("romaji"), m.get("idMal"))
        recent = m.pop("recent", {}).get("nodes", [])
        store.snapshot("anilist", "media", m["id"], m)
        for node in recent:
            store.trend(m["id"], node)
    store.commit()
    # start two months before the season so pre-air hype is included
    filled = _backfill_trends(http, store, [m["id"] for m in media],
                              season_start(year, season) - timedelta(days=60))
    return f"season={spec} media={len(media)} backfilled_trend_rows={filled}"


def excluded_media(m: dict) -> bool:
    """A work the game never lists, so the collector does not track it: adult (AniList isAdult or the Hentai genre,
    engine/economy.py Show.adult) or a PV / CM / teaser (engine/economy.py is_promo)."""
    from engine.economy import is_promo
    return bool(m.get("isAdult")) or "Hentai" in (m.get("genres") or []) or is_promo(m["id"], m)


def collect_anilist(store: Store, backfill: bool) -> str:
    # AniList allows 30 req/min right now; 2.1s spacing keeps us under it
    http = Http(min_interval=2.1, remaining_header="x-ratelimit-remaining",
                reset_header="x-ratelimit-reset")
    year, season = current_season(now_local().date())
    found: dict[int, dict] = {}
    for m in _anilist_pages(http, {"season": season.upper().replace("AUTUMN", "FALL"), "year": year}):
        found[m["id"]] = m
    for m in _anilist_pages(http, {"status": "RELEASING", "popGt": MIN_ANILIST_POPULARITY}):
        found.setdefault(m["id"], m)
    skipped = {mid for mid, m in found.items() if excluded_media(m)}
    found = {mid: m for mid, m in found.items() if mid not in skipped}
    for m in found.values():
        store.register("anilist", m["id"], m["title"].get("native") or m["title"].get("romaji"), m.get("idMal"))
    store.commit()

    # registry items that fell out of today's listings (finished shows in their long tail); one registered before the
    # exclusion is no longer snapshotted
    missing = [int(i) for i in store.tracked("anilist", tail=None) if int(i) not in found and int(i) not in skipped]
    for i in range(0, len(missing), 50):
        for m in _anilist_pages(http, {"ids": missing[i:i + 50]}):
            if not excluded_media(m):
                found[m["id"]] = m

    for mid, m in found.items():
        recent = m.pop("recent", {}).get("nodes", [])
        store.snapshot("anilist", "media", mid, m)
        for node in recent:
            store.trend(mid, node)
    store.commit()

    rel = collect_anilist_relations(http, store, list(found))

    filled = 0
    if backfill:
        filled = _backfill_trends(http, store, list(found), season_start(*previous_season(year, season)))
    return f"media={len(found)} missing_refetched={len(missing)} relations={rel} backfilled_trend_rows={filled}"


RELATIONS_QUERY = """
query ($ids: [Int], $page: Int) {
  Page(page: $page, perPage: 50) {
    pageInfo { hasNextPage }
    media(id_in: $ids, type: ANIME) {
      id
      relations {
        edges {
          relationType
          node { id type format status episodes popularity startDate { year month day } endDate { year month day }
                 title { native romaji } }
        }
      }
    }
  }
}
"""


def collect_anilist_relations(http: Http, store: Store, ids: list[int]) -> int:
    """Each tracked work's related entries (snapshot kind 'relations'): series grouping (docs/design/series.md §3.1) and
    sequel announcements, i.e. a new SEQUEL entry that has not aired yet (§5.2). A separate query keeps the main
    listing query under AniList's complexity limit."""
    n = 0
    for i in range(0, len(ids), 50):
        chunk = ids[i:i + 50]
        try:
            data = http.graphql(ANILIST, RELATIONS_QUERY, {"ids": chunk, "page": 1})
        except Exception as e:  # relations are an extra; the day's media snapshots are already stored
            log.warning("anilist relations %s..: %s", chunk[0], e)
            continue
        for m in data["Page"]["media"]:
            store.snapshot("anilist", "relations", m["id"], m["relations"])
            n += 1
        store.commit()
    return n


# ---------------------------------------------------------------- Annict

ANNICT = "https://api.annict.com/graphql"

WORK_FIELDS = """
  annictId title titleEn titleRo malAnimeId syobocalTid media seasonName seasonYear
  episodesCount watchersCount reviewsCount satisfactionRate noEpisodes
  officialSiteUrl twitterHashtag
"""

ANNICT_SEARCH = """
query ($seasons: [String!], $ids: [Int!], $after: String) {
  searchWorks(seasons: $seasons, annictIds: $ids, orderBy: {field: WATCHERS_COUNT, direction: DESC},
              first: 25, after: $after) {
    pageInfo { hasNextPage endCursor }
    nodes { %s }
  }
}
""" % WORK_FIELDS

ANNICT_EPISODES = """
query ($id: Int!, $after: String) {
  searchWorks(annictIds: [$id], first: 1) {
    nodes {
      episodes(orderBy: {field: SORT_NUMBER, direction: DESC}, first: 50, after: $after) {
        pageInfo { hasNextPage endCursor }
        nodes { annictId number numberText sortNumber title recordsCount recordCommentsCount satisfactionRate }
      }
    }
  }
}
"""


def annict_records_query(cursors: dict[int, str | None], first: int) -> tuple[str, dict]:
    """A page of records, newest first, for many episodes in one request: each episode under its own alias
    with its own cursor (Annict counts requests, not episodes)."""
    params = "".join(f", $a{eid}: String" for eid in cursors)
    parts = " ".join(
        f"e{eid}: searchEpisodes(annictIds: [{eid}]) {{ nodes {{ records(orderBy: {{field: CREATED_AT, direction: DESC}}, "
        f"first: $first, after: $a{eid}) {{ pageInfo {{ hasNextPage endCursor }} nodes {{ createdAt user {{ annictId }} }} }} }} }}"
        for eid in cursors)
    return f"query ($first: Int!{params}) {{ {parts} }}", {"first": first, **{f"a{e}": c for e, c in cursors.items()}}


# seconds per run for reading new records. A work's first read happens here too, a budget a day: Annict asks for
# full reads in pieces over hours or days (docs/design/data-terms.md). `collect.py --annict-viewers` reads everything at
# once, for a fresh database only; until a work is read its index falls back to recordsCount.
ANNICT_VIEWER_BUDGET_S = float(os.environ.get("ANNICT_VIEWER_BUDGET_S", "600"))
# one request at a time (Annict's rate limit page: 「リクエストは並列にせず、1件ずつ順番に送ってください」, made part of
# the API terms by Art. 11; docs/design/data-terms.md); each request still reads 20 episodes by alias
ANNICT_VIEWER_WORKERS, ANNICT_VIEWER_PER_MIN = 1, 300


@dataclass
class EpisodeRead:
    """One episode's records read since its last sync: each account's earliest record and the newest seen."""
    eid: int
    work_id: int
    count: int                      # the episode's recordsCount, stored once it is read
    seen: str | None                # newest_at of the last sync: reading stops once past it
    cursor: str | None = None
    newest: str | None = None
    firsts: dict[int, str] = field(default_factory=dict)
    read: int = 0

    def __post_init__(self):
        self.newest = self.seen

    def take(self, conn: dict) -> bool:
        """Adds one page; True when the episode is read back to its last sync."""
        for r in conn["nodes"]:
            at, user = r["createdAt"], r["user"]["annictId"]
            self.newest = max(self.newest or at, at)
            self.firsts[user] = min(self.firsts.get(user, at), at)
        self.read += len(conn["nodes"])
        self.cursor = conn["pageInfo"]["endCursor"]
        reached = self.seen is not None and conn["nodes"] and conn["nodes"][-1]["createdAt"] < self.seen
        return bool(reached) or not conn["pageInfo"]["hasNextPage"]


def read_records(http: "Http", todo: list[EpisodeRead], stop: Callable[[], bool],
                 done: Callable[[EpisodeRead], None], batch: int, page: int) -> bool:
    """Pages through the episodes `batch` at a time, one request per page of all of them, and hands each
    finished episode to `done`. False if `stop` said so before all were read."""
    waiting, active = deque(todo), {}
    while waiting or active:
        if stop():
            return False
        while waiting and len(active) < batch:
            ep = waiting.popleft()
            active[ep.eid] = ep
        data = http.graphql(ANNICT, *annict_records_query({e: ep.cursor for e, ep in active.items()}, page))
        for eid, ep in list(active.items()):
            nodes = (data.get(f"e{eid}") or {}).get("nodes")
            if not nodes:  # gone from Annict: no sync row, so it is tried again next run
                del active[eid]
            elif ep.take(nodes[0]["records"]):
                del active[eid]
                done(ep)
    return True


class Viewers:
    """Each Annict account counted once per episode (Annict lets an account record an episode again,
    docs/design/futures.md §3.1): reads the records added since the last run, newest first, a page of 20
    episodes per request."""

    BATCH, PAGE = 20, 100

    def __init__(self, http: "Http", store: Store, deadline: float):
        self.http, self.db, self.deadline = http, store.db, deadline

    def sync(self, work_id: int, episodes: list[dict]) -> int | None:
        """The work's distinct (account, episode) pairs, or None if time ran out before all were read."""
        if not read_records(self.http, self.stale(work_id, episodes), lambda: time.monotonic() > self.deadline,
                            self.save, self.BATCH, self.PAGE):
            return None
        ids = [e["annictId"] for e in episodes]
        if not ids:
            return 0
        return self.db.execute(f"SELECT COUNT(*) FROM annict_viewers WHERE episode_id IN ({','.join('?' * len(ids))})",
                               ids).fetchone()[0]

    def stale(self, work_id: int, episodes: list[dict]) -> list[EpisodeRead]:
        """The episodes whose recordsCount changed since their last sync."""
        out = []
        for e in episodes:
            row = self.db.execute("SELECT records, newest_at FROM annict_episode_sync WHERE episode_id=?",
                                  (e["annictId"],)).fetchone()
            if not row or row[0] != (e.get("recordsCount") or 0):
                out.append(EpisodeRead(e["annictId"], work_id, e.get("recordsCount") or 0, row[1] if row else None))
        return out

    def save(self, ep: EpisodeRead) -> None:
        """One transaction per episode: an interrupted sync resumes at the first episode not saved."""
        self.db.executemany("""INSERT INTO annict_viewers VALUES (?,?,?) ON CONFLICT(episode_id, user_id)
                               DO UPDATE SET first_at=min(first_at, excluded.first_at)""",
                            [(ep.eid, user, at) for user, at in ep.firsts.items()])
        self.db.execute("INSERT OR REPLACE INTO annict_episode_sync VALUES (?,?,?,?)",
                        (ep.eid, ep.work_id, ep.count, ep.newest))
        self.db.commit()

ANNICT_BY_TITLE = """
query ($titles: [String!]) {
  searchWorks(titles: $titles, orderBy: {field: WATCHERS_COUNT, direction: DESC}, first: 50) {
    nodes { %s }
  }
}
""" % WORK_FIELDS

def _flat(s: str | None) -> str:
    return re.sub(r"\s+", "", unicodedata.normalize("NFKC", s or "")).lower()


def season_near(work: dict, media: dict) -> bool:
    """Whether an Annict work's season starts within MAL_SEASON_MONTHS of the AniList show's premiere (true when
    either is unknown)."""
    from engine.economy import MAL_SEASON_MONTHS, SEASON_MONTH
    sd = media.get("startDate") or {}
    if not (sd.get("year") and work.get("seasonYear") and work.get("seasonName") in SEASON_MONTH):
        return True
    return abs(sd["year"] * 12 + (sd.get("month") or 1)
               - work["seasonYear"] * 12 - SEASON_MONTH[work["seasonName"]]) <= MAL_SEASON_MONTHS


DISCOVER_MIN_POPULARITY = 3000  # AniList list entries, the listing gate (game.min_listing_popularity)


def discover_annict(http: "Http", store: Store) -> int:
    """Annict works the season listing misses: long-runners (ONE PIECE, Conan...), ONAs and anything that
    started in an older season. For every airing or upcoming AniList show with a MAL id that no tracked
    Annict work carries, search Annict by title and keep a hit only when its MAL id matches. Works found
    earlier are re-registered every day so they stay tracked. Returns the number of new works."""
    by_mal = {int(m): i for i, m in store.db.execute(
        "SELECT external_id, mal_id FROM works WHERE source='annict' AND mal_id IS NOT NULL")}
    latest: dict[int, dict] = {}
    for i, d in store.db.execute(
            "SELECT external_id, data FROM snapshots WHERE source='anilist' AND kind='media' ORDER BY day"):
        latest[int(i)] = json.loads(d)
    new = 0
    for m in latest.values():
        mal = m.get("idMal")
        if not mal or m.get("status") not in ("RELEASING", "NOT_YET_RELEASED") \
                or (m.get("popularity") or 0) < DISCOVER_MIN_POPULARITY:
            continue
        if int(mal) in by_mal:
            store.register("annict", by_mal[int(mal)], None, mal)  # keep it tracked
            continue
        t = m.get("title") or {}
        seen: list[str] = []
        bare = re.sub(r"\s*[(（][^)）]*[)）]\s*$", "", t.get("native") or "")  # ドラえもん (2005) -> ドラえもん
        for title in dict.fromkeys(x for x in (t.get("native"), t.get("romaji"), bare) if x):
            try:
                nodes = http.graphql(ANNICT, ANNICT_BY_TITLE, {"titles": [title]})["searchWorks"]["nodes"]
            except Exception as e:  # one failed search must not stop the day's collection
                log.warning("annict title search failed for %s: %r", title, e)
                break
            seen += [f'{w["annictId"]} {w["title"]} mal={w.get("malAnimeId")}' for w in nodes[:5]]
            # the MAL id and a season near the premiere: MyAnimeList keeps some split cours as one entry, whose
            # later part AniList gives the earlier part's id (engine/economy.py MAL_SEASON_MONTHS)
            hit = next((w for w in nodes if w.get("malAnimeId") and int(w["malAnimeId"]) == int(mal)
                        and season_near(w, m)), None)
            if hit is None:
                # Annict leaves malAnimeId empty on some long-runners: accept the TV work whose title is exactly
                # the AniList native title (spacing and full-width forms ignored), only when it has no MAL id
                want = _flat(t.get("native"))
                hit = next((w for w in nodes if not w.get("malAnimeId") and w.get("media") == "TV"
                            and want and _flat(w.get("title")) == want), None)
            if hit:
                store.register("annict", hit["annictId"], hit["title"], mal)
                by_mal[int(mal)] = hit["annictId"]
                new += 1
                log.info("annict: found %s (%s) for AniList %s", hit["annictId"], hit["title"], m["id"])
                break
        else:
            log.info("annict: no MAL %s match for AniList %s %s; top results: %s",
                     mal, m["id"], t.get("native"), "; ".join(seen) or "none")
    store.commit()
    return new


FILMARKS = "https://filmarks.com"
FILMARKS_WORK_PAGES = 60        # work pages a day (episodes, review count, official site), in rotation: ~5 min
FILMARKS_MAX_PAGES = 10         # a season list has about 3 pages of 36


def _fm_json(attr: str, chunk: str) -> dict:
    import html as H
    m = re.search(attr + r'="([^"]*)"', chunk)
    return json.loads(H.unescape(m.group(1))) if m else {}


def _fm_text(cls: str, chunk: str) -> str | None:
    import html as H
    m = re.search(r'class="' + cls + r'"[^>]*>(.*?)</', chunk, re.S)
    return H.unescape(re.sub(r"<[^>]+>", "", m.group(1))).strip() if m else None


def parse_filmarks_list(page: str) -> list[dict]:
    """The works on one page of a Filmarks season list (/list-anime/release_year/YYYY/M): ids, title, release
    date, Mark count (watched and recorded), Clip count (観たい) and average score (None when unrated)."""
    out = []
    for chunk in page.split('<div class="js-cassette"')[1:]:
        mark, clip = _fm_json("data-mark", chunk), _fm_json("data-clip", chunk)
        if not mark.get("anime_season_id"):
            continue
        score = _fm_text("c-rating__score", chunk)
        released = re.search(r"公開日：</h4><span>(\d{4})年(\d{1,2})月(\d{1,2})日", chunk)
        out.append({
            "series_id": mark.get("anime_series_id"), "season_id": mark["anime_season_id"],
            "title": _fm_text("p-content-cassette__title", chunk),
            "released": "%s-%02d-%02d" % (released.group(1), int(released.group(2)), int(released.group(3))) if released else None,
            "marks": mark.get("count"), "clips": clip.get("count"),
            "score": float(score) if score and re.fullmatch(r"\d+(\.\d+)?", score) else None,
        })
    return out


def parse_filmarks_work(page: str) -> dict:
    """A Filmarks work page: per-episode comment and clap counts, the review count and rating of its JSON-LD,
    and the official site (for matching to Annict's officialSiteUrl)."""
    import html as H
    eps = []
    for item in page.split('class="c2-episode-list-item"')[1:]:
        m = re.search(r'href="/animes/\d+/\d+/episodes/(\d+)"><p>コメント([\d,]+)件</p><p>拍手([\d,]+)回</p>', item)
        if not m:
            continue
        title = re.search(r'header-text-title">(.*?)</div>', item)
        eps.append({"episode_id": int(m.group(1)), "comments": int(m.group(2).replace(",", "")),
                    "claps": int(m.group(3).replace(",", "")), "title": H.unescape(title.group(1)) if title else None})
    out: dict = {"episodes": eps}
    for block in re.findall(r'<script type="application/ld\+json">(.*?)</script>', page, re.S):
        try:
            ld = json.loads(block)
        except ValueError:
            continue
        if isinstance(ld, dict) and ld.get("@type") == "TVSeason":
            rating = ld.get("aggregateRating") or {}
            out.update(start=ld.get("startDate"), rating=rating.get("ratingValue"), reviews=rating.get("reviewCount"))
    site = re.search(r'links__item--official"><a[^>]*href="([^"]+)"', page)
    out["official"] = H.unescape(site.group(1)) if site else None
    return out


def filmarks_seasons(d: date) -> list[tuple[int, int]]:
    """The season lists to read on `d` (year, first month): the previous broadcast season's (its two-cour shows and
    finales' tails), the current one's and the next one's."""
    m = (d.month - 1) // 3 * 3 + 1
    prev = (d.year - (m == 1), 10 if m == 1 else m - 3)
    nxt = (d.year + (m == 10), 1 if m == 10 else m + 3)
    return [prev, (d.year, m), nxt]


def collect_filmarks(store: Store) -> str:
    """Filmarks anime counts, a candidate second Japan source (no API, no history: the series starts the day this
    runs). Every day the previous, current and next season lists (about 8 pages): Mark, Clip and score per work, a
    snapshot `filmarks/season` each. Then FILMARKS_WORK_PAGES work pages in rotation (works never read first):
    episodes' comment and clap counts, review count, official site, a snapshot `filmarks/work`. Only counts are kept,
    no review text (Filmarks terms §14, §19). Not used by the game yet; runs after the heat index (`main`), so its
    pages never delay the close."""
    http = Http(min_interval=5.0)
    seen: dict[int, dict] = {}
    pages = 0
    for year, month in filmarks_seasons(date.fromisoformat(store.day)):
        for n in range(1, FILMARKS_MAX_PAGES + 1):
            url = f"{FILMARKS}/list-anime/release_year/{year}/{month}" + (f"?page={n}" if n > 1 else "")
            r = http.request("GET", url)
            if r.status_code == 404:
                break
            r.raise_for_status()
            pages += 1
            works = parse_filmarks_list(r.text)
            for w in works:
                seen[w["season_id"]] = {**w, "list": f"{year}/{month}"}
            if not works or f"?page={n + 1}" not in r.text:
                break
    if not seen:
        raise RuntimeError(f"filmarks: no works parsed from {pages} pages (page layout changed?)")
    for sid, w in seen.items():
        store.register("filmarks", sid, w["title"])
        store.snapshot("filmarks", "season", sid, w)
    store.commit()
    last = dict(store.db.execute(
        "SELECT external_id, MAX(day) FROM snapshots WHERE source='filmarks' AND kind='work' GROUP BY 1").fetchall())
    # never read first, then the longest unread: each work about every 4 days at ~250 works
    todo = sorted(seen.values(), key=lambda w: last.get(str(w["season_id"]), ""))[:FILMARKS_WORK_PAGES]
    done = 0
    for w in todo:
        try:
            r = http.request("GET", f"{FILMARKS}/animes/{w['series_id']}/{w['season_id']}")
            r.raise_for_status()
            store.snapshot("filmarks", "work", w["season_id"], {"series_id": w["series_id"], **parse_filmarks_work(r.text)})
            store.commit()
            done += 1
        except Exception as e:  # one bad page must not lose the day's lists
            log.warning("filmarks work %s: %r", w["season_id"], e)
    return f"works={len(seen)} pages={pages} work_pages={done}/{len(todo)}"


def match_bangumi(store: Store) -> str:
    """Match the tracked AniList shows that have no Bangumi subject yet (engine/bgm_match.py): against the subjects
    collected so far, else Bangumi's search (once a week per show at most); a new match is registered for the daily
    Bangumi collection and snapshotted now."""
    from engine import bgm_match
    http = Http(min_interval=1.0)

    def search(keyword: str) -> list[dict]:
        r = http.request("POST", f"{BGM}/v0/search/subjects", params={"limit": 10},
                         json={"keyword": keyword, "filter": {"type": [2]}})
        r.raise_for_status()
        return r.json().get("data", [])

    def fetch(sid: int) -> dict | None:
        r = http.request("GET", f"{BGM}/v0/subjects/{sid}")
        return r.json() if r.status_code == 200 else None

    out = bgm_match.match_new(store.db, store.day, Path(__file__).parent / "sim" / "matches.json", search, fetch)
    for aid, sid in store.db.execute("SELECT anilist_id, bgm_id FROM bgm_matches WHERE matched_on=?", (store.day,)):
        store.register("bgm", sid, None)
        log.info("bgm: matched AniList %s -> subject %s", aid, sid)
    store.commit()
    return " ".join(f"{k}={v}" for k, v in out.items())


def annict_http(token: str, min_interval: float = 0.3) -> Http:
    return Http(min_interval=min_interval, remaining_header="ratelimit-remaining", reset_header="ratelimit-reset",
                headers={"Authorization": f"Bearer {token}"})


def collect_annict(store: Store, budget: float | None = None) -> str | None:
    token = os.environ.get("ANNICT_TOKEN", "").strip()
    if not token:
        return None
    http = annict_http(token)
    year, season = current_season(now_local().date())
    py, ps = previous_season(year, season)
    seasons = [f"{year}-{season}", f"{py}-{ps}"]

    found: dict[int, dict] = {}
    after = None
    while True:
        res = http.graphql(ANNICT, ANNICT_SEARCH, {"seasons": seasons, "after": after})["searchWorks"]
        stop = False
        for w in res["nodes"]:
            if w["watchersCount"] < MIN_ANNICT_WATCHERS:
                stop = True  # sorted by watchers desc, the rest is smaller
                break
            found[w["annictId"]] = w
        if stop or not res["pageInfo"]["hasNextPage"]:
            break
        after = res["pageInfo"]["endCursor"]
    for w in found.values():
        store.register("annict", w["annictId"], w["title"], w.get("malAnimeId"))
    store.commit()

    discovered = discover_annict(http, store)

    extra = [int(x) for x in os.environ.get("ANNICT_EXTRA_IDS", "").replace(" ", "").split(",") if x]
    missing = sorted(({int(i) for i in store.tracked("annict", tail=None)} | set(extra)) - set(found))
    for i in range(0, len(missing), 25):
        res = http.graphql(ANNICT, ANNICT_SEARCH, {"ids": missing[i:i + 25]})["searchWorks"]
        for w in res["nodes"]:
            found[w["annictId"]] = w
            store.register("annict", w["annictId"], w["title"], w.get("malAnimeId"))

    viewers = Viewers(http, store, time.monotonic() + (budget if budget is not None else ANNICT_VIEWER_BUDGET_S))
    pending = 0
    for wid, w in found.items():
        eps, after = [], None
        while len(eps) < 100:  # latest 100 episodes is plenty, even for long-runners
            r = http.graphql(ANNICT, ANNICT_EPISODES, {"id": wid, "after": after})["searchWorks"]["nodes"]
            if not r or not r[0]["episodes"]:
                break
            conn = r[0]["episodes"]
            eps.extend(conn["nodes"])
            if not conn["pageInfo"]["hasNextPage"]:
                break
            after = conn["pageInfo"]["endCursor"]
        pairs = viewers.sync(wid, eps)
        pending += pairs is None
        extra = {} if pairs is None else {"viewerPairs": pairs}
        store.snapshot("annict", "work", wid, {**w, "episodes": eps, **extra})
        store.commit()
    return f"works={len(found)} seasons={seasons} discovered={discovered} viewers_pending={pending}"


def sync_annict_viewers(store: Store) -> str:
    """One-off first sync of `annict_viewers` (no time limit; a few minutes) over the episodes of the latest
    Annict snapshots. Writes no snapshots: the next daily run reports viewerPairs."""
    token = os.environ["ANNICT_TOKEN"].strip()
    viewers = Viewers(None, store, float("inf"))
    day = store.db.execute("SELECT max(day) FROM snapshots WHERE source='annict' AND kind='work'").fetchone()[0]
    works = store.db.execute("SELECT external_id, data FROM snapshots WHERE source='annict' AND kind='work' AND day=?",
                             (day,)).fetchall()
    todo = [ep for wid, data in works for ep in viewers.stale(int(wid), json.loads(data).get("episodes") or [])]
    log.info("annict viewers: %d works, %d episodes to read", len(works), len(todo))
    n, records = _read_in_parallel(todo, token, viewers.save)
    pairs = store.db.execute("SELECT COUNT(*) FROM annict_viewers").fetchone()[0]
    return f"works={len(works)} episodes={n} records={records} pairs={pairs} (episodes of the {day} snapshots)"


def _read_in_parallel(todo: list[EpisodeRead], token: str, save: Callable[[EpisodeRead], None]) -> tuple[int, int]:
    """Workers read their share of the episodes; this thread alone writes, so the database never sees another
    thread. Episodes finished before a failure or Ctrl-C are still saved. Returns (episodes, records) read."""
    k, stop, finished = ANNICT_VIEWER_WORKERS, threading.Event(), queue.Queue()
    gap = 60.0 * k / ANNICT_VIEWER_PER_MIN
    t0, n, records = time.monotonic(), 0, 0

    def save_one(ep: EpisodeRead) -> None:
        nonlocal n, records
        save(ep)
        n, records = n + 1, records + ep.read
        if n % 200 == 0 or n == len(todo):
            secs = time.monotonic() - t0
            log.info("annict viewers: %d/%d episodes, %d records, %.0fs (%.0f records/s)",
                     n, len(todo), records, secs, records / max(secs, 1.0))

    with ThreadPoolExecutor(max_workers=k) as pool:
        futures = [pool.submit(read_records, annict_http(token, gap), todo[i::k], stop.is_set, finished.put,
                               Viewers.BATCH, Viewers.PAGE) for i in range(k)]
        try:
            while not all(f.done() for f in futures) and not any(f.done() and f.exception() for f in futures):
                try:
                    save_one(finished.get(timeout=1))
                except queue.Empty:
                    pass
        finally:
            stop.set()
            wait(futures)
            while not finished.empty():
                save_one(finished.get())
        for f in futures:
            f.result()
    return n, records


# ---------------------------------------------------------------- main

def print_stats(store: Store) -> None:
    q = store.db.execute
    print("works tracked:")
    for row in q("SELECT source, count(*) FROM works GROUP BY source"):
        print(f"  {row[0]:8} {row[1]}")
    print("snapshots by day (last 7):")
    for row in q("""SELECT day, source, kind, count(*) FROM snapshots
                    WHERE day >= date('now','-7 day') GROUP BY day, source, kind ORDER BY day DESC, source"""):
        print(f"  {row[0]} {row[1]:8} {row[2]:9} {row[3]}")
    r = q("SELECT count(*), count(DISTINCT media_id), min(date), max(date) FROM anilist_trends").fetchone()
    print(f"anilist_trends: rows={r[0]} media={r[1]} range={r[2]}..{r[3]}")
    print("last runs:")
    for row in q("SELECT source, started_at, status, detail FROM runs ORDER BY id DESC LIMIT 6"):
        print(f"  {row[0]:8} {row[1]} {row[2] or 'running':7} {row[3] or ''}")


def main() -> int:
    load_dotenv(Path(__file__).parent / ".env")
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--source", choices=["bgm", "anilist", "bgm_match", "annict", "filmarks", "all"], default="all")
    ap.add_argument("--backfill", action="store_true", help="backfill AniList trends history")
    ap.add_argument("--season", metavar="YYYY-season",
                    help="one-off: add a past AniList season (e.g. 2026-summer) and backfill its trends")
    ap.add_argument("--db", default=os.environ.get("ANISTOCK_DB", str(Path(__file__).parent / "data" / "anistock.db")))
    ap.add_argument("--stats", action="store_true")
    ap.add_argument("--annict-viewers", action="store_true",
                    help="one-off, fresh database only: read every Annict record at once (the daily run reads them a budget a day)")
    ap.add_argument("-v", "--verbose", action="store_true")
    args = ap.parse_args()
    logging.basicConfig(level=logging.DEBUG if args.verbose else logging.INFO,
                        format="%(asctime)s %(levelname)s %(message)s")
    logging.getLogger("httpx").setLevel(logging.WARNING)

    store = Store(Path(args.db))
    if args.stats:
        print_stats(store)
        return 0

    if args.annict_viewers:
        log.info("annict viewers: %s", sync_annict_viewers(store))
        return 0

    if args.season:
        run_id = store.run_start("anilist")
        try:
            detail = collect_anilist_season(store, args.season)
            store.run_end(run_id, "ok", detail)
            log.info("anilist season ok: %s", detail)
            return 0
        except Exception as e:
            store.run_end(run_id, "failed", repr(e))
            log.exception("anilist season failed")
            return 1

    jobs = {
        "bgm": lambda: collect_bangumi(store),
        "anilist": lambda: collect_anilist(store, args.backfill),
        "bgm_match": lambda: match_bangumi(store),
        "annict": lambda: collect_annict(store),
    }
    if args.source == "filmarks":
        jobs = {"filmarks": lambda: collect_filmarks(store)}
    failures = 0
    for name, fn in jobs.items():
        if args.source not in ("all", name):
            continue
        run_id = store.run_start(name)
        t0 = time.monotonic()
        try:
            detail = fn()
            if detail is None:
                store.run_end(run_id, "skipped", "ANNICT_TOKEN not set")
                log.info("%s skipped (ANNICT_TOKEN not set)", name)
            else:
                store.run_end(run_id, "ok", detail)
                log.info("%s ok in %.0fs: %s", name, time.monotonic() - t0, detail)
        except Exception as e:
            failures += 1
            store.run_end(run_id, "failed", repr(e))
            log.exception("%s failed", name)
    if args.source == "all":
        failures += after_collection()
        # last: a candidate source the game does not read yet, after the heat index so it never delays the close
        run_id = store.run_start("filmarks")
        try:
            detail = collect_filmarks(store)
            store.run_end(run_id, "ok", detail)
            log.info("filmarks ok: %s", detail)
        except Exception as e:
            failures += 1
            store.run_end(run_id, "failed", repr(e))
            log.exception("filmarks failed")
    return 1 if failures else 0


def after_collection() -> int:
    """After a full collection: the heat index in the background (engine/heat_index.py: every trading day whose
    next-day snapshots now exist), then the collection check (engine/collect_check.py; 1 if it fails, so the unit
    shows as failed). Each runs in its own process; neither stops the other."""
    import subprocess
    here = Path(__file__).parent
    bad = 0
    for script in ("engine/heat_index.py", "engine/collect_check.py"):
        r = subprocess.run([sys.executable, str(here / script)], cwd=here)
        if r.returncode != 0:
            log.error("%s exited with %d", script, r.returncode)
            bad += script.endswith("collect_check.py")
    return bad


if __name__ == "__main__":
    sys.exit(main())
