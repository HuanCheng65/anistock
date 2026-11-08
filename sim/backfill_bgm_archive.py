"""Backfill Bangumi's weekly collection counts from the official archive (github.com/bangumi/Archive releases),
for the heat index's cn market (docs/design/futures.md §3.6).

    uv run sim/backfill_bgm_archive.py [--since 2025-11-01] [--jobs 3] [--all-anime]

Each weekly dump (Tuesday ~21:00 UTC, Wednesday 05:00 Asia/Shanghai) carries every subject's `favorite` counts
(wish, done, doing, on_hold, dropped) and score. For every dump not read yet: download it (the .7z when there is
one, else the .zip) into a temporary file, stream subject.jsonlines out of it, keep the subjects mapped to an AniList
show (engine/economy.py effective_matches), and delete the download. The shared server's disk is nearly full: at
most --jobs archives at a time, nothing kept but the counts. GitHub is reached through the proxy (HTTPS_PROXY), about
4 MB/s a connection, so a few downloads run side by side; a dropped transfer resumes where it stopped.

Writes data/derived/heat_backfill.db, table bgm_weekly(subject_id, at, wish, done, doing, on_hold, dropped, score):
`at` is the dump's UTC time. The archive has no stated license: only these aggregate counts are kept and used.

--all-anime keeps every anime subject (type 2) instead of the mapped ones, into bgm_anime_weekly (same columns;
dumps read in bgm_anime_dumps), with each subject's names and air date in bgm_anime_subjects for title matching
(sim/match.py): one pass over the archive, so a newly mapped show never needs the dumps downloaded again.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sqlite3
import subprocess
import sys
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
OUT = Path(os.environ.get("ANISTOCK_DERIVED_DIR", ROOT / "data" / "derived")) / "heat_backfill.db"
NAME = re.compile(r"^dump-(\d{4}-\d{2}-\d{2})\.(\d{2})(\d{2})(\d{2})Z\.(7z|zip)$")


def assets() -> dict[str, dict[str, str]]:
    """dump UTC time -> {"7z": url, "zip": url}"""
    r = subprocess.run(["gh", "api", "repos/bangumi/Archive/releases", "--paginate", "-q",
                        ".[].assets[] | [.name, .browser_download_url] | @tsv"], capture_output=True, text=True, check=True)
    out: dict[str, dict[str, str]] = {}
    for line in r.stdout.splitlines():
        name, url = line.split("\t")
        m = NAME.match(name)
        if m:
            at = f"{m[1]}T{m[2]}:{m[3]}:{m[4]}Z"
            out.setdefault(at, {})[m[5]] = url
    return out


def subjects(path: Path, kind: str):
    """Lines of subject.jsonlines streamed out of the archive."""
    cmd = ["7z", "e", "-so", str(path), "subject.jsonlines"] if kind == "7z" else ["unzip", "-p", str(path), "subject.jsonlines"]
    p = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    yield from p.stdout
    p.wait()
    if p.returncode != 0:
        raise RuntimeError(f"{cmd[0]} exited with {p.returncode}")


def read_dump(at: str, urls: dict[str, str], wanted: set[int] | None) -> tuple[str, list[tuple], list[tuple], str]:
    """The dump's rows for `wanted` subjects (None: every anime), and the subjects' names and air dates."""
    kind = "7z" if "7z" in urls else "zip"
    t0 = time.monotonic()
    with tempfile.TemporaryDirectory(dir=OUT.parent) as tmp:
        path = Path(tmp) / f"dump.{kind}"
        for attempt in range(8):
            r = subprocess.run(["curl", "-sSL", "--retry", "5", "--retry-all-errors", "-C", "-", "-o", str(path), urls[kind]])
            if r.returncode == 0:
                break
            time.sleep(5 * (attempt + 1))
        else:
            raise RuntimeError(f"{at}: download failed")
        size = path.stat().st_size
        rows, names = [], []
        for line in subjects(path, kind):
            if '"type":2' not in line and '"type": 2' not in line:
                continue
            d = json.loads(line)
            if d.get("type") != 2 or (wanted is not None and d.get("id") not in wanted):
                continue
            f = d.get("favorite") or {}
            rows.append((d["id"], at, f.get("wish"), f.get("done"), f.get("doing"), f.get("on_hold"), f.get("dropped"), d.get("score")))
            if wanted is None:
                names.append((d["id"], d.get("name"), d.get("name_cn"), d.get("date"), d.get("platform"), at))
    return at, rows, names, f"{kind}, {size / 1e6:.0f} MB, {time.monotonic() - t0:.0f}s"


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--since", default="2025-11-01")
    ap.add_argument("--jobs", type=int, default=3)
    ap.add_argument("--all-anime", action="store_true", help="keep every anime subject (bgm_anime_*)")
    args = ap.parse_args()
    from engine import economy as E
    col = Path(os.environ.get("ANISTOCK_DB", ROOT / "data" / "anistock.db"))
    wanted = None if args.all_anime else {int(r["bgm"]) for r in E.effective_matches(col).values() if r.get("bgm")}
    weekly, dumps = ("bgm_anime_weekly", "bgm_anime_dumps") if args.all_anime else ("bgm_weekly", "bgm_dumps")
    OUT.parent.mkdir(parents=True, exist_ok=True)
    db = sqlite3.connect(OUT)
    db.execute(f"""CREATE TABLE IF NOT EXISTS {weekly} (subject_id INTEGER NOT NULL, at TEXT NOT NULL,
        wish INTEGER, done INTEGER, doing INTEGER, on_hold INTEGER, dropped INTEGER, score REAL,
        PRIMARY KEY (subject_id, at))""")
    db.execute(f"CREATE TABLE IF NOT EXISTS {dumps} (at TEXT PRIMARY KEY, subjects INTEGER NOT NULL)")
    db.execute("""CREATE TABLE IF NOT EXISTS bgm_anime_subjects (id INTEGER PRIMARY KEY, name TEXT, name_cn TEXT,
        date TEXT, platform INTEGER, seen_at TEXT NOT NULL)""")
    have = {r[0] for r in db.execute(f"SELECT at FROM {dumps}")}
    todo = sorted((at, urls) for at, urls in assets().items() if at[:10] >= args.since and at not in have)
    print(f"{'every anime subject' if wanted is None else f'{len(wanted)} mapped Bangumi subjects'}; {len(todo)} dumps to read, "
          f"{len(have)} read before", flush=True)
    failed = 0
    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        futs = [pool.submit(read_dump, at, urls, wanted) for at, urls in todo]
        for f in as_completed(futs):
            try:
                at, rows, names, note = f.result()
            except Exception as e:
                failed += 1
                print(f"  failed: {e}", flush=True)
                continue
            with db:
                db.executemany(f"INSERT OR IGNORE INTO {weekly} VALUES (?,?,?,?,?,?,?,?)", rows)
                db.execute(f"INSERT INTO {dumps} VALUES (?,?)", (at, len(rows)))
                # names as of the latest dump read
                db.executemany("""INSERT INTO bgm_anime_subjects VALUES (?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET
                    name=excluded.name, name_cn=excluded.name_cn, date=excluded.date, platform=excluded.platform,
                    seen_at=excluded.seen_at WHERE excluded.seen_at > seen_at""", names)
            print(f"  {at}: {len(rows)} subjects ({note})", flush=True)
    print("done" + (f", {failed} failed: run again to retry them" if failed else ""), flush=True)
    if failed:
        sys.exit(1)


if __name__ == "__main__":
    main()
