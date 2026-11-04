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
import sqlite3
import sys
import time
from datetime import date, datetime, timedelta, timezone
from pathlib import Path
from zoneinfo import ZoneInfo

import httpx

TZ = ZoneInfo("Asia/Shanghai")  # "day" boundaries are defined in this zone
USER_AGENT = "anistock-collector/0.1 (anime club market game)"
KEEP_TRACKING_DAYS = 180  # keep snapshotting a work this long after it was last discovered
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

    def tracked(self, source: str) -> list[str]:
        cutoff = (date.fromisoformat(self.day) - timedelta(days=KEEP_TRACKING_DAYS)).isoformat()
        rows = self.db.execute(
            "SELECT external_id FROM works WHERE source=? AND last_discovered>=?", (source, cutoff))
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
    store.commit()

    ids = store.tracked("bgm")
    ok = failed = 0
    for sid in ids:
        try:
            r = http.request("GET", f"{BGM}/v0/subjects/{sid}")
            if r.status_code == 404:
                log.warning("bgm subject %s not found", sid)
                continue
            r.raise_for_status()
            store.snapshot("bgm", "subject", sid, r.json())

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
    return f"discovered={discovered} tracked={len(ids)} ok={ok} failed={failed}"


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
  genres
  tags { name rank }
  stats { statusDistribution { status amount } scoreDistribution { score amount } }
"""

PAGE_QUERY = """
query ($page: Int, $season: MediaSeason, $year: Int, $status: MediaStatus, $popGt: Int, $ids: [Int]) {
  Page(page: $page, perPage: 50) {
    pageInfo { hasNextPage }
    media(type: ANIME, season: $season, seasonYear: $year, status: $status,
          popularity_greater: $popGt, id_in: $ids, sort: POPULARITY_DESC) {
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
    for m in found.values():
        store.register("anilist", m["id"], m["title"].get("native") or m["title"].get("romaji"), m.get("idMal"))
    store.commit()

    # registry items that fell out of today's listings (finished shows in their long tail)
    missing = [int(i) for i in store.tracked("anilist") if int(i) not in found]
    for i in range(0, len(missing), 50):
        for m in _anilist_pages(http, {"ids": missing[i:i + 50]}):
            found[m["id"]] = m

    for mid, m in found.items():
        recent = m.pop("recent", {}).get("nodes", [])
        store.snapshot("anilist", "media", mid, m)
        for node in recent:
            store.trend(mid, node)
    store.commit()

    filled = 0
    if backfill:
        cutoff = int(datetime.combine(season_start(*previous_season(year, season)),
                                      datetime.min.time(), timezone.utc).timestamp())
        for mid in found:
            page = 1
            while True:
                t = http.graphql(ANILIST, TRENDS_QUERY, {"id": mid, "page": page})["Media"]["trends"]
                for node in t["nodes"]:
                    store.trend(mid, node)
                    filled += 1
                store.commit()
                oldest = min((n["date"] for n in t["nodes"]), default=0)
                if not t["pageInfo"]["hasNextPage"] or oldest < cutoff:
                    break
                page += 1
            log.info("anilist backfill media %s done", mid)
    return f"media={len(found)} missing_refetched={len(missing)} backfilled_trend_rows={filled}"


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


def collect_annict(store: Store) -> str | None:
    token = os.environ.get("ANNICT_TOKEN", "").strip()
    if not token:
        return None
    http = Http(min_interval=0.3, remaining_header="ratelimit-remaining", reset_header="ratelimit-reset",
                headers={"Authorization": f"Bearer {token}"})
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

    extra = [int(x) for x in os.environ.get("ANNICT_EXTRA_IDS", "").replace(" ", "").split(",") if x]
    missing = sorted(({int(i) for i in store.tracked("annict")} | set(extra)) - set(found))
    for i in range(0, len(missing), 25):
        res = http.graphql(ANNICT, ANNICT_SEARCH, {"ids": missing[i:i + 25]})["searchWorks"]
        for w in res["nodes"]:
            found[w["annictId"]] = w
            store.register("annict", w["annictId"], w["title"], w.get("malAnimeId"))

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
        store.snapshot("annict", "work", wid, {**w, "episodes": eps})
        store.commit()
    return f"works={len(found)} seasons={seasons}"


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
    ap.add_argument("--source", choices=["bgm", "anilist", "annict", "all"], default="all")
    ap.add_argument("--backfill", action="store_true", help="backfill AniList trends history")
    ap.add_argument("--db", default=os.environ.get("ANISTOCK_DB", str(Path(__file__).parent / "data" / "anistock.db")))
    ap.add_argument("--stats", action="store_true")
    ap.add_argument("-v", "--verbose", action="store_true")
    args = ap.parse_args()
    logging.basicConfig(level=logging.DEBUG if args.verbose else logging.INFO,
                        format="%(asctime)s %(levelname)s %(message)s")
    logging.getLogger("httpx").setLevel(logging.WARNING)

    store = Store(Path(args.db))
    if args.stats:
        print_stats(store)
        return 0

    jobs = {
        "bgm": lambda: collect_bangumi(store),
        "anilist": lambda: collect_anilist(store, args.backfill),
        "annict": lambda: collect_annict(store),
    }
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
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
