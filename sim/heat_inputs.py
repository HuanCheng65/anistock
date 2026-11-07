"""Replay inputs with the heat index (docs/v3-futures-impl.md step 11): a copy of an inputs directory whose files
carry the top-level `heat` the futures settle on, from the backfilled history (sim/heat_history.py: AniList from
the collector, Bangumi's weekly archive, Annict's record times; table heat_history of data/derived/heat_backfill.db).

    uv run python sim/heat_inputs.py INPUTS_DIR OUT_DIR [--scale S]

The index is computed as the live one is (engine/heat_index.py), with the method's constants
(engine/heat_index.py, v1), so a replay settles on what the real index would have read.
"""

from __future__ import annotations

import argparse
import json
import sqlite3
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
from engine.heat_index import PLATFORM, SCALE, WEIGHTS  # noqa: E402  (the method's constants, v1)


def load(db: Path, scale: float) -> dict[str, dict[str, dict]]:
    """day -> work -> the inputs' heat entry."""
    out: dict[str, dict[str, dict]] = {}
    con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    for work, day, us, cn, jp in con.execute("SELECT work_id, day, us, cn, jp FROM heat_history"):
        vals = {"us": us, "cn": cn, "jp": jp}
        idx = sum(WEIGHTS[m] * v / PLATFORM[m] * scale for m, v in vals.items() if v is not None)
        flags = {m: "missing" for m, v in vals.items() if v is None}
        out.setdefault(day, {})[str(work)] = {**vals, "idx": idx, "flags": flags, "version": "backfill"}
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("inputs", type=Path)
    ap.add_argument("out", type=Path)
    ap.add_argument("--db", type=Path, default=ROOT / "data" / "derived" / "heat_backfill.db")
    ap.add_argument("--scale", type=float, default=SCALE)
    args = ap.parse_args()
    heat = load(args.db, args.scale)
    args.out.mkdir(parents=True, exist_ok=True)
    n = with_heat = 0
    for f in sorted(args.inputs.glob("*.json")):
        raw = json.loads(f.read_text())
        day = heat.get(raw["day"])
        if day:
            raw["heat"] = {k: v for k, v in day.items() if any(str(s["id"]) == k for s in raw["shows"])}
            with_heat += 1
        (args.out / f.name).write_text(json.dumps(raw, ensure_ascii=False, separators=(",", ":")))
        n += 1
    print(f"{n} files, {with_heat} with the index -> {args.out}")


if __name__ == "__main__":
    main()
