"""Copy heat index days the server did not publish itself into the game database (docs/design/futures.md §3).

    uv run python -m server.tools.import_heat [--from data/derived/heat_index.db] [--game data/game.db] [--dry-run]
    uv run python -m server.tools.import_heat --backfill heat_backfill.csv.gz      # the year before the live index
    uv run python -m server.tools.import_heat --export-backfill heat_backfill.csv.gz   # on the machine with the backfills

The collector computes the index every day (engine/heat_index.py, data/derived/heat_index.db); the server
publishes a day into the game database's `heat_index` once that day's inputs carry it. The default run copies
the live days computed before that (from the first collected day to the deploy).

The live index needs a snapshot on each side of a day, so it starts when the collector started taking them.
`--backfill` adds the history before: each work's days from sim/heat_history.py (AniList trends, Bangumi's weekly
archive spread over the week, Annict's records), turned into the index with the method v1 constants, version
`v1-backfill`, only for days before the work's first live value. A day missing a market the work has on other
days is left out rather than drawn low. The backfill is not published-and-frozen like the live index: each
`--backfill` run replaces the backfilled rows of an earlier one (the live rows are never touched). The stock page draws them lighter; futures
never use them (a contract's days are all after its listing). `--export-backfill` writes that history as a small
gzipped CSV (work_id, day, us, cn, jp) from data/derived/heat_backfill.db, to copy to the server.

Rows already in the game database are kept (published values are frozen), so running it twice changes nothing.
Safe while the server runs: plain inserts in one transaction; the server reads past days from the database.
"""

from __future__ import annotations

import argparse
import csv
import gzip
import os
import sqlite3
import sys
from pathlib import Path

from server.config import ROOT, load

COLS = "work_id, day, idx, us, cn, jp, flags, version"
DERIVED = Path(os.environ.get("ANISTOCK_DERIVED_DIR", ROOT / "data" / "derived"))  # as engine/heat_index.py


def index_v1(us: float | None, cn: float | None, jp: float | None) -> float:
    """engine/heat_index.py's index with the method v1 constants (markets without a source left out)."""
    from engine.heat_index import PLATFORM, SCALE, WEIGHTS
    vals = {"us": us, "cn": cn, "jp": jp}
    return sum(WEIGHTS[m] * max(0.0, v) / PLATFORM[m] * SCALE for m, v in vals.items() if v is not None)


def live_rows(src: str) -> list[tuple]:
    db = sqlite3.connect(f"file:{src}?mode=ro", uri=True)
    rows = db.execute(f"SELECT {COLS} FROM heat_index ORDER BY day, work_id").fetchall()
    versions = sorted({r[7] for r in rows})
    if versions != ["v1"]:
        print(f"warning: expected only method v1 rows, found {versions or 'none'}; recompute first "
              "(mv heat_index.db aside, uv run engine/heat_index.py)", file=sys.stderr)
    return rows


def backfill_rows(path: str, game: sqlite3.Connection) -> list[tuple]:
    """The complete backfilled days before each work's first live value in the game database."""
    first = dict(game.execute("SELECT work_id, MIN(day) FROM heat_index WHERE version NOT LIKE '%backfill' GROUP BY work_id"))
    num = lambda x: float(x) if x not in (None, "") else None  # noqa: E731
    with gzip.open(path, "rt", newline="") as f:
        days = [(int(r["work_id"]), r["day"], num(r["us"]), num(r["cn"]), num(r["jp"])) for r in csv.DictReader(f)]
    # the markets each work has on some day: a day without one of them is incomplete
    has: dict[int, set[int]] = {}
    for w, _, *vals in days:
        has.setdefault(w, set()).update(i for i, v in enumerate(vals) if v is not None)
    out = []
    for w, day, us, cn, jp in days:
        if w in first and day >= first[w]:
            continue
        if any((us, cn, jp)[i] is None for i in has[w]):
            continue
        out.append((w, day, round(index_v1(us, cn, jp), 6), us, cn, jp, "{}", "v1-backfill"))
    return out


def drop_backfill(game: sqlite3.Connection) -> int:
    """Remove the backfilled rows of an earlier import (never a live row)."""
    with game:
        return game.execute("DELETE FROM heat_index WHERE version LIKE '%backfill'").rowcount


def export_backfill(out: str) -> int:
    db = sqlite3.connect(f"file:{DERIVED / 'heat_backfill.db'}?mode=ro", uri=True)
    rows = db.execute("SELECT work_id, day, us, cn, jp FROM heat_history ORDER BY work_id, day").fetchall()
    with gzip.open(out, "wt", newline="") as f:
        w = csv.writer(f)
        w.writerow(["work_id", "day", "us", "cn", "jp"])
        for r in rows:
            w.writerow([r[0], r[1]] + ["" if v is None else f"{v:.4f}" for v in r[2:]])
    print(f"wrote {len(rows)} days of {len({r[0] for r in rows})} works to {out}")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--from", dest="src", default=str(DERIVED / "heat_index.db"))
    ap.add_argument("--backfill", metavar="CSV_GZ", help="the history before the live index (--export-backfill's file)")
    ap.add_argument("--export-backfill", metavar="CSV_GZ")
    ap.add_argument("--game")
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()
    if args.export_backfill:
        return export_backfill(args.export_backfill)

    game = sqlite3.connect(args.game or str(load().game_db), timeout=30)
    if not game.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='heat_index'").fetchone():
        print("the game database has no heat_index table yet: start the new server once (it migrates), then run this",
              file=sys.stderr)
        return 1
    rows = backfill_rows(args.backfill, game) if args.backfill else live_rows(args.src)
    if args.backfill and not args.dry_run:
        dropped = drop_backfill(game)
        if dropped:
            print(f"replacing {dropped} backfilled rows of an earlier import")
    have = {(w, d) for w, d in game.execute("SELECT work_id, day FROM heat_index")}
    new = [r for r in rows if (r[0], r[1]) not in have]
    days = sorted({r[1] for r in new})
    print(f"{len(rows)} rows from {args.backfill or args.src}; {len(new)} new"
          + (f", {days[0]} to {days[-1]} ({len(days)} days, {len({r[0] for r in new})} works)" if days else ""))
    if args.dry_run or not new:
        return 0
    with game:
        game.executemany(f"INSERT OR IGNORE INTO heat_index({COLS}) VALUES (?,?,?,?,?,?,?,?)", new)
    print(f"imported {len(new)} rows")
    return 0


if __name__ == "__main__":
    sys.exit(main())
