# /// script
# requires-python = ">=3.11"
# dependencies = ["httpx[socks]>=0.27"]
# ///
"""Backfill multi-year attention history for the long-tail and sequel study (sim/calibration/long_tail_sequel.md).

    .venv/bin/python sim/history_backfill.py DB catalog          # AniList TV/ONA 2018-2025, popularity >= 3000 (+relations)
    .venv/bin/python sim/history_backfill.py DB trends           # in-season daily + 14-day windows 1..96 months after finale
    .venv/bin/python sim/history_backfill.py DB firsttrend       # first mediaTrends day of every sequel entry (announcement proxy)
    .venv/bin/python sim/history_backfill.py DB announce         # daily window +-45 days around each sequel's first trend day
    .venv/bin/python sim/history_backfill.py DB bgm --map BANGUMI_DATA_JSON [--sample 500]
                                                                 # Bangumi: subject, episode list, comments on 3 episodes,
                                                                 # sampled pages of subject (collection) comments

Every step is resumable: finished units are recorded in the DB and skipped on a re-run. Writes only to DB
(its own SQLite file, never data/*.db). Only timestamps and counts are stored, no user names or comment text.
AniList ~28 requests/min; Bangumi 1 request/s. Uses $https_proxy when set.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sqlite3
import sys
import time
from datetime import date, datetime, timedelta, timezone

import httpx

UA = "anistock-collector/0.1 (anime club market game)"
JST = timezone(timedelta(hours=9))
AL = "https://graphql.anilist.co"
TAIL_MONTHS = [1, 2, 3, 4.5, 6, 9, 12, 15, 18, 21, 24, 30, 36, 42, 48, 60, 72, 84, 96]
TREND_FIELDS = "date trending popularity inProgress averageScore episode"
SEQUEL_TYPES = ("SEQUEL",)

SCHEMA = """
CREATE TABLE IF NOT EXISTS media (id INTEGER PRIMARY KEY, in_scope INTEGER NOT NULL, data TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS trends (media_id INTEGER, day TEXT, trending INTEGER, popularity INTEGER,
    in_progress INTEGER, score INTEGER, episode INTEGER, PRIMARY KEY (media_id, day));
CREATE TABLE IF NOT EXISTS windows_done (media_id INTEGER, tag TEXT, start TEXT, end TEXT, n INTEGER,
    PRIMARY KEY (media_id, tag));
CREATE TABLE IF NOT EXISTS first_trend (media_id INTEGER PRIMARY KEY, day TEXT, popularity INTEGER);
CREATE TABLE IF NOT EXISTS done (step TEXT, key TEXT, PRIMARY KEY (step, key));
CREATE TABLE IF NOT EXISTS bgm_map (anilist_id INTEGER PRIMARY KEY, bgm_id INTEGER);
CREATE TABLE IF NOT EXISTS bgm_subject (bgm_id INTEGER PRIMARY KEY, data TEXT, episodes TEXT);
CREATE TABLE IF NOT EXISTS bgm_ep_comments (episode_id INTEGER PRIMARY KEY, bgm_id INTEGER, sort REAL,
    airdate TEXT, n_eps INTEGER, times TEXT);
CREATE TABLE IF NOT EXISTS bgm_subject_comments (bgm_id INTEGER, offset INTEGER, total INTEGER, times TEXT,
    PRIMARY KEY (bgm_id, offset));
"""


def connect(path: str) -> sqlite3.Connection:
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    db = sqlite3.connect(path, timeout=120)
    db.executescript(SCHEMA)
    return db


def client() -> httpx.Client:
    proxy = os.environ.get("https_proxy") or None
    return httpx.Client(proxy=proxy, headers={"User-Agent": UA, "Accept": "application/json"}, timeout=60)


class AniList:
    def __init__(self):
        self.c = client()
        self.last = 0.0

    def q(self, query: str, variables: dict | None = None) -> dict:
        for attempt in range(8):
            wait = 2.15 - (time.time() - self.last)
            if wait > 0:
                time.sleep(wait)
            self.last = time.time()
            try:
                r = self.c.post(AL, json={"query": query, "variables": variables or {}})
            except httpx.HTTPError as e:
                print(f"  anilist net error {e!r}, retry", flush=True)
                time.sleep(10 * (attempt + 1))
                continue
            if r.status_code == 429:
                ra = int(r.headers.get("retry-after", "60"))
                print(f"  429, sleeping {ra}s", flush=True)
                time.sleep(ra + 1)
                continue
            if r.status_code >= 500:
                time.sleep(10 * (attempt + 1))
                continue
            j = r.json()
            if j.get("errors") and not j.get("data"):
                raise RuntimeError(j["errors"])
            return j["data"]
        raise RuntimeError("anilist: too many retries")


def jst_midnight(d: date) -> int:
    return int(datetime(d.year, d.month, d.day, tzinfo=JST).timestamp())


def fdate(d: dict | None) -> date | None:
    if not d or not d.get("year") or not d.get("month"):
        return None
    return date(d["year"], d["month"], d.get("day") or 15)


# ---------------------------------------------------------------- catalog

CAT = """query($a:FuzzyDateInt,$b:FuzzyDateInt,$p:Int){ Page(perPage:50,page:$p){ pageInfo{hasNextPage}
 media(type:ANIME, format_in:[TV,ONA], startDate_greater:$a, startDate_lesser:$b, popularity_greater:2999,
       sort:POPULARITY_DESC){
 id idMal title{romaji native english} format status episodes duration source genres countryOfOrigin isAdult
 averageScore meanScore popularity favourites startDate{year month day} endDate{year month day} season seasonYear
 studios(isMain:true){nodes{id name}} tags{name rank}
 relations{edges{relationType node{id type format status episodes chapters volumes popularity
   startDate{year month day} endDate{year month day} title{romaji}}}} } } }"""


def cmd_catalog(db, a):
    al = AniList()
    for y in range(2018, 2026):
        if db.execute("SELECT 1 FROM done WHERE step='catalog' AND key=?", (str(y),)).fetchone():
            continue
        p, n = 1, 0
        while True:
            pg = al.q(CAT, {"a": y * 10000, "b": (y + 1) * 10000, "p": p})["Page"]
            for m in pg["media"]:
                db.execute("INSERT OR REPLACE INTO media VALUES (?,1,?)", (m["id"], json.dumps(m)))
            n += len(pg["media"])
            if not pg["pageInfo"]["hasNextPage"]:
                break
            p += 1
        db.execute("INSERT INTO done VALUES ('catalog',?)", (str(y),))
        db.commit()
        print(f"catalog {y}: {n}", flush=True)
    print("in scope:", db.execute("SELECT count(*) FROM media WHERE in_scope=1").fetchone()[0])


# ---------------------------------------------------------------- trend windows

def show_windows(m: dict, today: date) -> list[tuple[str, date, date]]:
    """(tag, first day, last day) windows for one show: daily in-season, then 14-day windows after the finale."""
    s, e = fdate(m["startDate"]), fdate(m["endDate"])
    if not s:
        return []
    out = []
    end_in = (e + timedelta(days=14)) if e and m["status"] == "FINISHED" else today - timedelta(days=2)
    begin = max(s - timedelta(days=7), end_in - timedelta(days=400))
    d, k = begin, 0
    while d <= end_in:
        last = min(d + timedelta(days=49), end_in)
        out.append((f"s{k}", d, last))
        d, k = last + timedelta(days=1), k + 1
    if e and m["status"] == "FINISHED":
        for mo in TAIL_MONTHS:
            a = e + timedelta(days=round(mo * 30.44))
            if a + timedelta(days=13) > today - timedelta(days=2):
                break
            out.append((f"t{mo}", a, a + timedelta(days=13)))
    return out


def fetch_windows(db, al, jobs: list[tuple[int, str, date, date]], per_request: int = 40):
    """jobs: (media_id, tag, first, last). Packs up to per_request aliases per request; pages >50-day windows never
    occur (windows are <= 50 days)."""
    todo = [j for j in jobs if not db.execute("SELECT 1 FROM windows_done WHERE media_id=? AND tag=?",
                                              (j[0], j[1])).fetchone()]
    print(f"windows to fetch: {len(todo)} (of {len(jobs)}), ~{len(todo) // per_request + 1} requests", flush=True)
    for i in range(0, len(todo), per_request):
        batch = todo[i:i + per_request]
        parts = []
        for k, (mid, tag, a, b) in enumerate(batch):
            n = (b - a).days + 1
            parts.append(f"w{k}: Page(perPage:{max(n, 1)}){{ mediaTrends(mediaId:{mid}, sort:DATE, "
                         f"date_greater:{jst_midnight(a) - 1}, date_lesser:{jst_midnight(b) + 1}){{{TREND_FIELDS}}} }}")
        data = al.q("{" + "\n".join(parts) + "}")
        for k, (mid, tag, a, b) in enumerate(batch):
            rows = (data.get(f"w{k}") or {}).get("mediaTrends") or []
            for t in rows:
                day = datetime.fromtimestamp(t["date"], JST).date().isoformat()
                db.execute("INSERT OR REPLACE INTO trends VALUES (?,?,?,?,?,?,?)",
                           (mid, day, t["trending"], t["popularity"], t["inProgress"], t["averageScore"], t["episode"]))
            db.execute("INSERT OR REPLACE INTO windows_done VALUES (?,?,?,?,?)",
                       (mid, tag, a.isoformat(), b.isoformat(), len(rows)))
        db.commit()
        print(f"  {min(i + per_request, len(todo))}/{len(todo)}", flush=True)


def cmd_trends(db, a):
    today = date.today()
    jobs = []
    for mid, data in db.execute("SELECT id, data FROM media WHERE in_scope=1 ORDER BY id"):
        for tag, s, e in show_windows(json.loads(data), today):
            jobs.append((mid, tag, s, e))
    fetch_windows(db, AniList(), jobs)


# ---------------------------------------------------------------- sequels

def sequel_ids(db) -> set[int]:
    ids = set()
    for (data,) in db.execute("SELECT data FROM media WHERE in_scope=1"):
        for ed in json.loads(data)["relations"]["edges"]:
            n = ed["node"]
            if ed["relationType"] in SEQUEL_TYPES and n["type"] == "ANIME":
                ids.add(n["id"])
    return ids


def cmd_firsttrend(db, a):
    al = AniList()
    ids = sorted(i for i in sequel_ids(db) if not db.execute("SELECT 1 FROM first_trend WHERE media_id=?", (i,)).fetchone())
    print(f"sequel entries without first trend: {len(ids)}", flush=True)
    for i in range(0, len(ids), 40):
        batch = ids[i:i + 40]
        parts = [f"f{mid}: Page(perPage:1){{ mediaTrends(mediaId:{mid}, sort:DATE){{date popularity}} }}" for mid in batch]
        data = al.q("{" + "\n".join(parts) + "}")
        for mid in batch:
            rows = (data.get(f"f{mid}") or {}).get("mediaTrends") or []
            day = datetime.fromtimestamp(rows[0]["date"], JST).date().isoformat() if rows else None
            db.execute("INSERT OR REPLACE INTO first_trend VALUES (?,?,?)", (mid, day, rows[0]["popularity"] if rows else None))
        db.commit()
        print(f"  {min(i + 40, len(ids))}/{len(ids)}", flush=True)


def cmd_announce(db, a):
    """Daily windows on the ORIGINAL show around each sequel's first trend day (announcement proxy), when that day is
    after the original's finale + 30 days."""
    ft = dict(db.execute("SELECT media_id, day FROM first_trend WHERE day IS NOT NULL"))
    today = date.today()
    jobs = []
    for mid, data in db.execute("SELECT id, data FROM media WHERE in_scope=1 ORDER BY id"):
        m = json.loads(data)
        e = fdate(m["endDate"])
        if not e or m["status"] != "FINISHED":
            continue
        for ed in m["relations"]["edges"]:
            sid = ed["node"]["id"]
            if ed["relationType"] not in SEQUEL_TYPES or sid not in ft:
                continue
            A = date.fromisoformat(ft[sid])
            if A < e + timedelta(days=30) or A + timedelta(days=46) > today:
                continue
            jobs.append((mid, f"a{sid}_0", A - timedelta(days=45), A - timedelta(days=1)))
            jobs.append((mid, f"a{sid}_1", A, A + timedelta(days=45)))
    fetch_windows(db, AniList(), jobs)


# ---------------------------------------------------------------- Bangumi

class Bgm:
    def __init__(self):
        self.c = client()
        self.last = 0.0

    def get(self, url: str) -> dict | list | None:
        for attempt in range(6):
            wait = 1.0 - (time.time() - self.last)
            if wait > 0:
                time.sleep(wait)
            self.last = time.time()
            try:
                r = self.c.get(url)
            except httpx.HTTPError as e:
                print(f"  bgm net error {e!r}", flush=True)
                time.sleep(10 * (attempt + 1))
                continue
            if r.status_code == 404:
                return None
            if r.status_code in (429,) or r.status_code >= 500:
                time.sleep(15 * (attempt + 1))
                continue
            if r.status_code != 200:
                print(f"  bgm {r.status_code} {url}", flush=True)
                return None
            return r.json()
        return None


def cmd_bgm(db, a):
    items = json.load(open(a.map, encoding="utf-8"))["items"]
    for it in items:
        s = {x["site"]: x.get("id") for x in it.get("sites", []) if x.get("id")}
        if "aniList" in s and "bangumi" in s:
            try:
                db.execute("INSERT OR IGNORE INTO bgm_map VALUES (?,?)", (int(s["aniList"]), int(s["bangumi"])))
            except ValueError:
                pass
    db.commit()
    rows = db.execute("""SELECT m.id, b.bgm_id, json_extract(m.data,'$.startDate.year') FROM media m
                         JOIN bgm_map b ON b.anilist_id=m.id WHERE m.in_scope=1""").fetchall()
    print(f"scope shows with a Bangumi id: {len(rows)}", flush=True)
    # deterministic stratified sample: per start year, ordered by a hash of the id
    by_year: dict[int, list] = {}
    for mid, bid, y in rows:
        by_year.setdefault(y, []).append((hashlib.md5(str(mid).encode()).hexdigest(), bid))
    for v in by_year.values():
        v.sort()
    order = []
    k = 0
    while any(k < len(v) for v in by_year.values()):
        for y in sorted(by_year):
            if k < len(by_year[y]):
                order.append(by_year[y][k][1])
        k += 1
    order = order[:a.sample]
    bg = Bgm()
    for n, bid in enumerate(order):
        if db.execute("SELECT 1 FROM done WHERE step='bgm' AND key=?", (str(bid),)).fetchone():
            continue
        subj = bg.get(f"https://api.bgm.tv/v0/subjects/{bid}")
        eps, off = [], 0
        while True:
            j = bg.get(f"https://api.bgm.tv/v0/episodes?subject_id={bid}&type=0&limit=100&offset={off}")
            if not j:
                break
            eps += [{"id": e["id"], "sort": e["sort"], "airdate": e.get("airdate"), "comment": e.get("comment", 0)}
                    for e in j.get("data", [])]
            off += 100
            if off >= j.get("total", 0):
                break
        if subj is not None:
            keep = {k: subj.get(k) for k in ("id", "name", "date", "platform", "eps", "total_episodes", "rating",
                                             "collection")}
            db.execute("INSERT OR REPLACE INTO bgm_subject VALUES (?,?,?)", (bid, json.dumps(keep), json.dumps(eps)))
        aired = sorted((e for e in eps if e["airdate"]), key=lambda e: e["sort"])
        if aired:
            picks = {aired[0]["id"]: aired[0], aired[len(aired) // 2]["id"]: aired[len(aired) // 2],
                     aired[-1]["id"]: aired[-1]}
            for eid, e in picks.items():
                if db.execute("SELECT 1 FROM bgm_ep_comments WHERE episode_id=?", (eid,)).fetchone():
                    continue
                js = bg.get(f"https://next.bgm.tv/p1/episodes/{eid}/comments")
                if js is None:
                    continue
                times = sorted([x["createdAt"] for x in js] + [y["createdAt"] for x in js for y in x.get("replies", [])])
                db.execute("INSERT OR REPLACE INTO bgm_ep_comments VALUES (?,?,?,?,?,?)",
                           (eid, bid, e["sort"], e["airdate"], len(aired), json.dumps(times)))
        # subject (collection) comments, newest first; sample pages at offsets 0, 200, 400, 800, ... and the last page
        j = bg.get(f"https://next.bgm.tv/p1/subjects/{bid}/comments?limit=100&offset=0")
        if j is not None:
            total = j.get("total", 0)
            db.execute("INSERT OR REPLACE INTO bgm_subject_comments VALUES (?,?,?,?)",
                       (bid, 0, total, json.dumps([x["updatedAt"] for x in j.get("data", [])])))
            offs, o = [], 200
            while o < total - 100:
                offs.append(o)
                o *= 2
            if total > 100:
                offs.append(total - 100)
            for o in offs:
                jj = bg.get(f"https://next.bgm.tv/p1/subjects/{bid}/comments?limit=100&offset={o}")
                if jj is None:
                    continue
                db.execute("INSERT OR REPLACE INTO bgm_subject_comments VALUES (?,?,?,?)",
                           (bid, o, jj.get("total", total), json.dumps([x["updatedAt"] for x in jj.get("data", [])])))
        db.execute("INSERT INTO done VALUES ('bgm',?)", (str(bid),))
        db.commit()
        if n % 10 == 0:
            print(f"  bgm {n + 1}/{len(order)}", flush=True)
    print("bgm done", flush=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("db")
    ap.add_argument("step", choices=["catalog", "trends", "firsttrend", "announce", "bgm"])
    ap.add_argument("--map", help="bangumi-data dist/data.json (for bgm)")
    ap.add_argument("--sample", type=int, default=500)
    a = ap.parse_args()
    db = connect(a.db)
    {"catalog": cmd_catalog, "trends": cmd_trends, "firsttrend": cmd_firsttrend, "announce": cmd_announce,
     "bgm": cmd_bgm}[a.step](db, a)


if __name__ == "__main__":
    sys.exit(main())
