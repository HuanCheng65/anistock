"""AniList shows -> Bangumi subjects by title, against the subjects of the Bangumi archive (sim/backfill_bgm_archive.py
--all-anime, table bgm_anime_subjects): for backfilling the heat index's cn market over shows the collector never
tracked (docs/design/futures.md §3.7). Only shows without a match yet (engine/economy.py effective_matches); a show
matches a subject whose Japanese or Chinese name normalises to one of its titles (sim/match.py `norm`) and that
first aired within 60 days of it, the nearest date winning; failing that, a name one of its titles is a prefix of
(or the reverse; 4 characters or more) in the same window; failing that, the same title without its season
marker or number. Ambiguous or dateless candidates are left out.

    ANISTOCK_DB=data/fast/collector_b.db uv run python sim/match_archive.py [--out data/derived/archive_matches.json]

Calibration only: sim/matches.json (what the game uses) is not changed.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sqlite3
import sys
from collections import defaultdict
from datetime import date
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / "sim"))
from engine import economy as E  # noqa: E402
from match import anilist_start, anilist_titles, norm  # noqa: E402

DERIVED = Path(os.environ.get("ANISTOCK_DERIVED_DIR", ROOT / "data" / "derived"))


def bare(t: str) -> str:
    """A normalised title without its trailing season marker, number or year (at least 3 characters left)."""
    x = re.sub(r"(S\d+|\d+|[弐参肆])+$", "", re.sub(r"(19|20)\d\d(?=S\d+$|$)", "", t))
    return x if len(x) >= 3 else ""


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--out", type=Path, default=DERIVED / "archive_matches.json")
    args = ap.parse_args()
    col_path = Path(os.environ.get("ANISTOCK_DB", ROOT / "data" / "anistock.db"))
    col = sqlite3.connect(f"file:{col_path}?mode=ro", uri=True)
    media = {int(i): json.loads(d) for i, d in col.execute(
        "SELECT external_id, data FROM snapshots WHERE source='anilist' AND kind='media' ORDER BY day")}
    matched = {k for k, v in E.effective_matches(col_path).items() if v.get("bgm")}
    by_name: dict[str, list[tuple[int, date | None]]] = defaultdict(list)
    for sid, name, name_cn, d in sqlite3.connect(DERIVED / "heat_backfill.db").execute(
            "SELECT id, name, name_cn, date FROM bgm_anime_subjects"):
        try:
            when = date.fromisoformat(d) if d else None
        except ValueError:
            when = None
        for n in {norm(name), norm(name_cn)} - {""}:
            by_name[n].append((sid, when))
    out, ambiguous, missing = {}, 0, 0
    for mid, m in media.items():
        if str(mid) in matched:
            continue
        start = anilist_start(m)
        titles = anilist_titles(m)
        stems = {bare(t) for t in titles} - {""}

        def near(found):
            return sorted((abs((when - start).days), sid) for sid, when in found if start and when and abs((when - start).days) <= 60)

        # each rule only when the stricter one before it found nothing in the window
        cands = near({(sid, when) for t in titles for sid, when in by_name.get(t, [])})
        if not cands:
            # a sequel's marker written differently (ワンパンマン３ / ワンパンマン 第3期): one title a prefix of the other
            cands = near({(sid, when) for name, subs in by_name.items() if len(name) >= 4
                          for t in titles if len(t) >= 4 and (name.startswith(t) or t.startswith(name)) for sid, when in subs})
        if not cands:
            # the same title without its season marker or number (かくりよの宿飯 第２期 / かくりよの宿飯 弐)
            cands = near({(sid, when) for name, subs in by_name.items() if bare(name) in stems for sid, when in subs})
        if not cands:
            missing += 1
        elif len(cands) > 1 and cands[0][0] == cands[1][0] and cands[0][1] != cands[1][1]:
            ambiguous += 1
        else:
            out[str(mid)] = cands[0][1]
    args.out.write_text(json.dumps(out, indent=1, sort_keys=True))
    print(f"{len(media)} shows, {len(matched)} matched before; {len(out)} matched now, {ambiguous} ambiguous, "
          f"{missing} without a candidate -> {args.out}")


if __name__ == "__main__":
    main()
