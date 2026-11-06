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
import time
from collections import Counter
from datetime import datetime, timedelta, timezone
from pathlib import Path

import httpx

ROOT = Path(__file__).resolve().parent.parent
OUT = Path(os.environ.get("ANISTOCK_DERIVED_DIR", ROOT / "data" / "derived")) / "heat_backfill.db"
TZ = timezone(timedelta(hours=8))
QUERY = """query($id: Int!, $after: String) {
  searchEpisodes(annictIds: [$id]) { nodes {
    records(first: 100, after: $after, orderBy: {field: CREATED_AT, direction: ASC}) {
      pageInfo { hasNextPage endCursor } nodes { createdAt } } } } }"""


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

    def query(self, variables: dict) -> dict:
        for attempt in range(6):
            wait = self.last + self.gap - time.monotonic()
            if wait > 0:
                time.sleep(wait)
            self.last = time.monotonic()
            self.n += 1
            try:
                r = self.http.post("https://api.annict.com/graphql", json={"query": QUERY, "variables": variables})
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
        raise RuntimeError(f"giving up on {variables}")


def main() -> None:
    load_dotenv()
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--per-min", type=int, default=300)
    args = ap.parse_args()
    token = os.environ.get("ANNICT_TOKEN")
    if not token:
        sys.exit("ANNICT_TOKEN is not set (.env)")
    col = sqlite3.connect(f"file:{os.environ.get('ANISTOCK_DB', ROOT / 'data' / 'anistock.db')}?mode=ro", uri=True)
    day = col.execute("SELECT MAX(day) FROM snapshots WHERE source='annict' AND kind='work'").fetchone()[0]
    works = [(int(ext), json.loads(data)) for ext, data in col.execute(
        "SELECT external_id, data FROM snapshots WHERE source='annict' AND kind='work' AND day=?", (day,))]
    OUT.parent.mkdir(parents=True, exist_ok=True)
    db = sqlite3.connect(OUT)
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
    c = Client(token, args.per_min)
    t0 = time.monotonic()
    counted = 0
    for k, (work, ep) in enumerate(todo, start=1):
        per_day: Counter = Counter()
        after, n = None, 0
        while True:
            data = c.query({"id": ep["annictId"], "after": after})
            nodes = data["searchEpisodes"]["nodes"]
            if not nodes:
                break
            rec = nodes[0]["records"]
            for r in rec["nodes"]:
                t = datetime.fromisoformat(r["createdAt"].replace("Z", "+00:00")).astimezone(TZ)
                per_day[t.date().isoformat()] += 1
                n += 1
            if not rec["pageInfo"]["hasNextPage"]:
                break
            after = rec["pageInfo"]["endCursor"]
        with db:
            for d, x in per_day.items():
                db.execute("INSERT INTO annict_daily VALUES (?,?,?) ON CONFLICT(annict_id, day) DO UPDATE SET records = records + excluded.records",
                           (work, d, x))
            db.execute("INSERT INTO annict_done VALUES (?,?,?,?)", (ep["annictId"], work, n, datetime.now(TZ).isoformat(timespec="seconds")))
        counted += n
        if k % 50 == 0 or k == len(todo):
            rate = counted / max(1.0, time.monotonic() - t0)
            print(f"  {k}/{len(todo)} episodes, {counted} records, {c.n} requests, {time.monotonic() - t0:.0f}s ({rate:.0f} records/s)")
    print("done")


if __name__ == "__main__":
    main()
