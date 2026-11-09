"""Rebuild the us platform's daily size (docs/design/series.md §6) over the trailing year from a collector database that
holds every airing work's AniList trends, for `revenue.platform_history` (engine/daily_inputs.py `platform_history`).

The server's collector started on 2026-11-05 and backfilled the trends of the works it met then, so its own days before
that miss every show that had already finished: its July-September 2026 platform was ~20% low and June near empty, and
the trailing mean would have risen for a year as those days left the window, cutting every show's us revenue. A
separate collector database filled with the collector's own season backfill (`collect.py --season` for each season of
the year, then the trends of everything airing) measures it the way the live one does.

    ANISTOCK_DB=data/derived/platform_hist/collector.db uv run python sim/platform_history.py --from 2025-11-01 --until 2026-11-08

Writes data/derived/platform_us.json (or --out): {"note", "us": {day: total}}; copy it to the server's data/derived/.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from datetime import date, timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from engine import daily_inputs as D  # noqa: E402
from engine import economy as E  # noqa: E402


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--from", dest="start", required=True)
    ap.add_argument("--until", required=True, help="first day not written (the collector's last complete day + 1)")
    ap.add_argument("--out", type=Path, default=Path(os.environ.get("ANISTOCK_DERIVED_DIR", ROOT / "data" / "derived")) / "platform_us.json")
    a = ap.parse_args()
    from server.config import load
    p = load().params
    db = Path(os.environ.get("ANISTOCK_DB", ROOT / "data" / "anistock.db"))
    start, until = date.fromisoformat(a.start), date.fromisoformat(a.until)
    # load from a season before, so a show airing on `start` has its episode history (the 14-day airing rule)
    shows, _ = E.load_shows(db, p, start - timedelta(days=120), until)
    tot = D.airing_totals(shows, {}, p["revenue"], start, until)
    days = {d.isoformat(): round(t["us"], 3) for d, t in sorted(tot.items())}
    missing = [(start + timedelta(days=k)).isoformat() for k in range((until - start).days)
               if (start + timedelta(days=k)).isoformat() not in days]
    if missing:
        sys.exit(f"days without any airing work: {missing[:5]}... ({len(missing)}): the collector database is incomplete")
    a.out.write_text(json.dumps({"note": f"us platform's daily size, {len(shows)} works of {db.name} (sim/platform_history.py)",
                                 "us": days}, indent=0) + "\n", encoding="utf-8")
    print(f"{len(days)} days {a.start}..{a.until} -> {a.out}")


if __name__ == "__main__":
    main()
