"""Operator's sequel overrides (docs/design/series.md §5.4), read by engine/daily_inputs.py when it builds a day's
inputs: an official sequel announcement AniList has not listed yet, or an official statement that there will be no
sequel. Only for an official source (the anime's site or account, the publisher); never for rumours or leaks.

    uv run engine/sequel_override.py announce WORK_ID --source URL [--premiere 2027-04] [--day YYYY-MM-DD]
    uv run engine/sequel_override.py cancel WORK_ID --source URL [--day YYYY-MM-DD]
    uv run engine/sequel_override.py remove WORK_ID
    uv run engine/sequel_override.py list

WORK_ID: the AniList id of the series' latest work (the one whose sequel it is). --day: the close it takes effect
at (default: the next close, the trading day after the last inputs file). The close posts the notice with the
source and, for an announcement, raises the production cost; an announcement keeps its sequel id once AniList lists
the entry, so neither happens twice. The file is data/sequel_overrides.json (ANISTOCK_SEQUEL_OVERRIDES).
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from datetime import date, datetime, timedelta, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from engine.daily_inputs import inputs_dir, load_relations, sequel_overrides_path  # noqa: E402


def next_close() -> str:
    days = sorted(f.stem for f in inputs_dir().glob("????-??-??.json"))
    if days:
        return (date.fromisoformat(days[-1]) + timedelta(days=1)).isoformat()
    return datetime.now(timezone(timedelta(hours=8))).date().isoformat()


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    for name in ("announce", "cancel"):
        a = sub.add_parser(name)
        a.add_argument("work", type=int)
        a.add_argument("--source", required=True, help="the official announcement (URL)")
        a.add_argument("--day", help="the close it takes effect at (default: the next close)")
        if name == "announce":
            a.add_argument("--premiere", help="YYYY, YYYY-MM or YYYY-MM-DD if announced")
    sub.add_parser("remove").add_argument("work", type=int)
    sub.add_parser("list")
    a = ap.parse_args()
    path = sequel_overrides_path()
    ov = json.loads(path.read_text(encoding="utf-8")) if path.exists() else {}
    if a.cmd == "list":
        for k, v in sorted(ov.items()):
            print(k, json.dumps(v, ensure_ascii=False))
        return
    key = str(a.work)
    if a.cmd == "remove":
        if ov.pop(key, None) is None:
            sys.exit(f"{key}: no override")
    else:
        if not re.match(r"^https?://\S+$", a.source):
            sys.exit("--source: a URL")
        day = a.day or next_close()
        date.fromisoformat(day)
        entry = {"source": a.source, "added": datetime.now(timezone.utc).isoformat(timespec="seconds")}
        if a.cmd == "announce":
            if a.premiere and not re.match(r"^\d{4}(-\d{2}(-\d{2})?)?$", a.premiere):
                sys.exit("--premiere: YYYY, YYYY-MM or YYYY-MM-DD")
            rel = load_relations(Path(__import__("server.config", fromlist=["load"]).load().collector_db))
            snaps = rel.get(a.work)
            if snaps and any(e["relationType"] == "SEQUEL" and e["node"].get("status") == "NOT_YET_RELEASED"
                             for e in snaps[-1][1]):
                sys.exit(f"{key}: AniList already lists an unaired sequel; no override needed")
            entry.update(announced=day, **({"premiere": a.premiere} if a.premiere else {}))
        else:
            entry.update(cancelled=day)
        ov[key] = entry
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(json.dumps(ov, ensure_ascii=False, indent=1), encoding="utf-8")
    tmp.replace(path)
    print(f"{path}: {key} {json.dumps(ov.get(key), ensure_ascii=False)}")


if __name__ == "__main__":
    main()
