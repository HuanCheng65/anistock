"""Copy the heat index computed before the server published it into the game database (docs/v3-futures.md §3).

    uv run python -m server.tools.import_heat [--from data/derived/heat_index.db] [--game data/game.db] [--dry-run]

The collector computes the index every day (engine/heat_index.py, data/derived/heat_index.db); the server
publishes a day into the game database's `heat_index` once that day's inputs carry it. The days before that
(from the first collected day to the deploy) are only in the derived database: this copies them, so the stock
page's 热度 tab starts with them. Rows already in the game database are kept (published values are frozen),
so running it twice changes nothing. Safe while the server runs: plain inserts in one transaction, and the
server reads past days from the database, not from memory.
"""

from __future__ import annotations

import argparse
import os
import sqlite3
import sys
from pathlib import Path

from server.config import ROOT, load

COLS = "work_id, day, idx, us, cn, jp, flags, version"


def main() -> int:
    cfg = load()
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    derived = Path(os.environ.get("ANISTOCK_DERIVED_DIR", ROOT / "data" / "derived"))  # as engine/heat_index.py
    ap.add_argument("--from", dest="src", default=str(derived / "heat_index.db"))
    ap.add_argument("--game", default=str(cfg.game_db))
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    src = sqlite3.connect(f"file:{args.src}?mode=ro", uri=True)
    rows = src.execute(f"SELECT {COLS} FROM heat_index ORDER BY day, work_id").fetchall()
    versions = sorted({r[7] for r in rows})
    game = sqlite3.connect(args.game, timeout=30)
    if not game.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='heat_index'").fetchone():
        print("the game database has no heat_index table yet: start the new server once (it migrates), then run this", file=sys.stderr)
        return 1
    have = {(w, d) for w, d in game.execute("SELECT work_id, day FROM heat_index")}
    new = [r for r in rows if (r[0], r[1]) not in have]
    days = sorted({r[1] for r in new})
    print(f"{len(rows)} rows in {args.src} (versions {', '.join(versions) or '-'}); {len(new)} new"
          + (f", {days[0]} to {days[-1]} ({len(days)} days)" if days else ""))
    if "v1" not in versions or len(versions) > 1:
        print("warning: expected only method v1 rows; recompute first (mv heat_index.db aside, uv run engine/heat_index.py)",
              file=sys.stderr)
    if args.dry_run or not new:
        return 0
    with game:
        game.executemany(f"INSERT OR IGNORE INTO heat_index({COLS}) VALUES (?,?,?,?,?,?,?,?)", new)
    print(f"imported {len(new)} rows")
    return 0


if __name__ == "__main__":
    sys.exit(main())
