"""Backfill Annict's daily episode records for the heat index (docs/v3-futures.md §3.6: calibrate on history
instead of waiting weeks for live snapshots).

    uv run sim/backfill_annict_records.py [--per-min 300]

Every Annict work the collector tracks (its latest `annict work` snapshot) and every one of its episodes: page
through the episode's records (100 at a time, oldest first) and count them per Asia/Shanghai calendar day by
`createdAt`. A record's time is when the viewer logged it, so a day's count is exactly the change of the work's
summed `recordsCount` the live index measures (less records deleted since, which the live count also loses).

Writes data/derived/heat_backfill.db, table annict_daily(annict_id, day, records). Resumable: an episode whose
records were all counted is noted in annict_done and skipped on the next run. Needs ANNICT_TOKEN (GraphQL);
paced to --per-min requests (Annict allows 600 a minute per token).
"""

from __future__ import annotations

import argparse
import json
import os
import sqlite3
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from collections import Counter
from datetime import datetime, timedelta, timezone
from pathlib import Path

import httpx

ROOT = Path(__file__).resolve().parent.parent
OUT = Path(os.environ.get("ANISTOCK_DERIVED_DIR", ROOT / "data" / "derived")) / "heat_backfill.db"
TZ = timezone(timedelta(hours=8))
# Annict counts requests: one request reads a page of records of many episodes, each by alias with its own cursor
BATCH = 20


def batch_query(cursors: dict[int, str | None]) -> str:
    parts = [f'e{eid}: searchEpisodes(annictIds: [{eid}]) {{ nodes {{ records(first: 100, after: {json.dumps(after)}, '
             f'orderBy: {{field: CREATED_AT, direction: ASC}}) {{ pageInfo {{ hasNextPage endCursor }} nodes {{ createdAt }} }} }} }}'
             for eid, after in cursors.items()]
    return "query { " + " ".join(parts) + " }"


def load_dotenv() -> None:
    p = ROOT / ".env"
    if p.exists():
        for line in p.read_text().splitlines():
            if "=" in line and not line.lstrip().startswith("#"):
                k, v = line.split("=", 1)
                os.environ.setdefault(k.strip(), v.strip())


class Client:
    def __init__(self, token: str, per_min: int):
        self.http = httpx.Client(timeout=60, headers={"Authorization": f"Bearer {token}"})
        self.gap = 60.0 / per_min
        self.last = 0.0
        self.n = 0

    def query(self, query: str) -> dict:
        for attempt in range(6):
            wait = self.last + self.gap - time.monotonic()
            if wait > 0:
                time.sleep(wait)
            self.last = time.monotonic()
            self.n += 1
            try:
                r = self.http.post("https://api.annict.com/graphql", json={"query": query})
            except httpx.HTTPError as e:
                print(f"  network error ({e}), retrying", file=sys.stderr)
                time.sleep(5 * (attempt + 1))
                continue
            if r.status_code in (429, 500, 502, 503, 504):
                time.sleep(10 * (attempt + 1))
                continue
            r.raise_for_status()
            body = r.json()
            if body.get("errors"):
                raise RuntimeError(json.dumps(body["errors"])[:500])
            return body["data"]
        raise RuntimeError(f"giving up on {query[:200]}")


def main() -> None:
    load_dotenv()
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--per-min", type=int, default=300)
    ap.add_argument("--workers", type=int, default=1)  # Annict: requests one at a time (docs/v3-data-terms.md)
    args = ap.parse_args()
    token = os.environ.get("ANNICT_TOKEN")
    if not token:
        sys.exit("ANNICT_TOKEN is not set (.env)")
    col = sqlite3.connect(f"file:{os.environ.get('ANISTOCK_DB', ROOT / 'data' / 'anistock.db')}?mode=ro", uri=True)
    day = col.execute("SELECT MAX(day) FROM snapshots WHERE source='annict' AND kind='work'").fetchone()[0]
    works = [(int(ext), json.loads(data)) for ext, data in col.execute(
        "SELECT external_id, data FROM snapshots WHERE source='annict' AND kind='work' AND day=?", (day,))]
    OUT.parent.mkdir(parents=True, exist_ok=True)
    db = sqlite3.connect(OUT, check_same_thread=False)
    db.executescript("""
        CREATE TABLE IF NOT EXISTS annict_daily (annict_id INTEGER NOT NULL, day TEXT NOT NULL, records INTEGER NOT NULL,
            PRIMARY KEY (annict_id, day));
        CREATE TABLE IF NOT EXISTS annict_done (episode_id INTEGER PRIMARY KEY, annict_id INTEGER NOT NULL,
            records INTEGER NOT NULL, at TEXT NOT NULL);""")
    done = {r[0] for r in db.execute("SELECT episode_id FROM annict_done")}
    todo = [(w, e) for w, d in works for e in (d.get("episodes") or []) if e.get("annictId") not in done and (e.get("recordsCount") or 0) > 0]
    total = sum(e.get("recordsCount") or 0 for _, e in todo)
    print(f"{len(works)} works ({day}); {len(todo)} episodes with records to count, about {total} records, "
          f"{len(done)} episodes done before")
    t0 = time.monotonic()
    # a big request takes the server seconds: `--workers` read side by side, each with its own episodes and its
    # share of the rate limit; the database writes are serialised
    lock = threading.Lock()
    stats = {"finished": 0, "counted": 0, "requests": 0}

    def work(chunk: list) -> None:
        cl = Client(token, max(1, args.per_min // args.workers))
        queue = list(chunk)
        active: dict[int, list] = {}   # episode -> [work, cursor, records per day, records]
        while queue or active:
            while queue and len(active) < BATCH:
                w, ep = queue.pop(0)
                active[ep["annictId"]] = [w, None, Counter(), 0]
            data = cl.query(batch_query({eid: a[1] for eid, a in active.items()}))
            for eid in list(active):
                w, _, per_day, n = active[eid]
                nodes = (data.get(f"e{eid}") or {}).get("nodes") or []
                rec = nodes[0]["records"] if nodes else {"nodes": [], "pageInfo": {"hasNextPage": False}}
                for r in rec["nodes"]:
                    t = datetime.fromisoformat(r["createdAt"].replace("Z", "+00:00")).astimezone(TZ)
                    per_day[t.date().isoformat()] += 1
                    n += 1
                active[eid][3] = n
                if rec["pageInfo"]["hasNextPage"]:
                    active[eid][1] = rec["pageInfo"]["endCursor"]
                    continue
                del active[eid]
                with lock, db:
                    for d, x in per_day.items():
                        db.execute("INSERT INTO annict_daily VALUES (?,?,?) ON CONFLICT(annict_id, day) DO UPDATE SET records = records + excluded.records",
                                   (w, d, x))
                    db.execute("INSERT INTO annict_done VALUES (?,?,?,?)", (eid, w, n, datetime.now(TZ).isoformat(timespec="seconds")))
                    stats["finished"] += 1
                    stats["counted"] += n
                    if stats["finished"] % 200 == 0 or stats["finished"] == len(todo):
                        rate = stats["counted"] / max(1.0, time.monotonic() - t0)
                        print(f"  {stats['finished']}/{len(todo)} episodes, {stats['counted']} records, "
                              f"{time.monotonic() - t0:.0f}s ({rate:.0f} records/s)", flush=True)

    with ThreadPoolExecutor(max_workers=args.workers) as pool:
        list(pool.map(work, [todo[k::args.workers] for k in range(args.workers)]))
    print("done")


if __name__ == "__main__":
    main()
