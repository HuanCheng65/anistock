"""Backfill Bangumi's weekly collection counts from the official archive (github.com/bangumi/Archive releases),
for the heat index's cn market (docs/v3-futures.md §3.6).

    uv run sim/backfill_bgm_archive.py [--since 2025-06-01]

Each weekly dump (Tuesday ~21:00 UTC, Wednesday 05:00 Asia/Shanghai) carries every subject's `favorite` counts
(wish, done, doing, on_hold, dropped) and score. For every dump not read yet: download it (the .7z when there is
one, else the .zip) into a temporary file, stream subject.jsonlines out of it, keep the subjects mapped to an AniList
show (engine/economy.py effective_matches), and delete the download. The shared server's disk is nearly full: one
archive at a time, nothing kept but the counts.

Writes data/derived/heat_backfill.db, table bgm_weekly(subject_id, at, wish, done, doing, on_hold, dropped, score):
`at` is the dump's UTC time. The archive has no stated license: only these aggregate counts are kept and used.
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


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--since", default="2025-06-01")
    args = ap.parse_args()
    from engine import economy as E
    col = Path(os.environ.get("ANISTOCK_DB", ROOT / "data" / "anistock.db"))
    wanted = {int(r["bgm"]) for r in E.effective_matches(col).values() if r.get("bgm")}
    OUT.parent.mkdir(parents=True, exist_ok=True)
    db = sqlite3.connect(OUT)
    db.execute("""CREATE TABLE IF NOT EXISTS bgm_weekly (subject_id INTEGER NOT NULL, at TEXT NOT NULL,
        wish INTEGER, done INTEGER, doing INTEGER, on_hold INTEGER, dropped INTEGER, score REAL,
        PRIMARY KEY (subject_id, at))""")
    db.execute("CREATE TABLE IF NOT EXISTS bgm_dumps (at TEXT PRIMARY KEY, subjects INTEGER NOT NULL)")
    have = {r[0] for r in db.execute("SELECT at FROM bgm_dumps")}
    todo = sorted((at, urls) for at, urls in assets().items() if at[:10] >= args.since and at not in have)
    print(f"{len(wanted)} mapped Bangumi subjects; {len(todo)} dumps to read, {len(have)} read before")
    for at, urls in todo:
        kind = "7z" if "7z" in urls else "zip"
        t0 = time.monotonic()
        with tempfile.TemporaryDirectory(dir=OUT.parent) as tmp:
            path = Path(tmp) / f"dump.{kind}"
            subprocess.run(["curl", "-sSL", "--retry", "3", "-o", str(path), urls[kind]], check=True)
            size = path.stat().st_size
            rows = []
            for line in subjects(path, kind):
                if '"type":2' not in line and '"type": 2' not in line:
                    continue
                d = json.loads(line)
                if d.get("id") in wanted:
                    f = d.get("favorite") or {}
                    rows.append((d["id"], at, f.get("wish"), f.get("done"), f.get("doing"), f.get("on_hold"), f.get("dropped"), d.get("score")))
        with db:
            db.executemany("INSERT OR IGNORE INTO bgm_weekly VALUES (?,?,?,?,?,?,?,?)", rows)
            db.execute("INSERT INTO bgm_dumps VALUES (?,?)", (at, len(rows)))
        print(f"  {at}: {len(rows)} subjects ({kind}, {size / 1e6:.0f} MB, {time.monotonic() - t0:.0f}s)", flush=True)
    print("done")


if __name__ == "__main__":
    main()
