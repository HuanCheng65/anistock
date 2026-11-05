"""Series grouping (docs/v2-series.md §3.1): one company per series.

Default rule: works connected by SEQUEL / PREQUEL relations among TV, TV_SHORT, ONA and MOVIE entries are one series
(through entries the game does not track, too: a first season outside the data still links its second and third).
Side stories, spin-offs and alternatives are not joined. The exception table `sim/series.json` (in git, kept by hand
like sim/matches.json) can:

    "subsidiary": {"<AniList id>": "<parent id>", ...}   list a work as its own company (its audience does not move
                                                        with the main line); the parent is shown on its profile
    "merge":      [["<id>", "<id>", ...], ...]          join works AniList does not link (a renamed reboot)

A series' key is the smallest AniList id in it (deterministic, independent of what is listed). Pure functions; the
relations come from the collector's AniList 'relations' snapshots (engine/daily_inputs.py `load_relations`).
"""

from __future__ import annotations

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SERIES_FORMATS = {"TV", "TV_SHORT", "ONA", "MOVIE"}
LINKS = {"SEQUEL", "PREQUEL"}


def load_exceptions(path: Path | None = None) -> dict:
    f = path or ROOT / "sim" / "series.json"
    raw = json.loads(f.read_text(encoding="utf-8")) if f.exists() else {}
    return {"subsidiary": {int(k): int(v) for k, v in raw.get("subsidiary", {}).items()},
            "merge": [[int(x) for x in g] for g in raw.get("merge", [])]}


def series_keys(edges_by_id: dict[int, list], ids, exceptions: dict | None = None) -> dict[int, int]:
    """{id: series key} for every id in `ids`. edges_by_id: each work's AniList relation edges (latest snapshot)."""
    ex = exceptions or {"subsidiary": {}, "merge": []}
    sub = ex["subsidiary"]
    parent: dict[int, int] = {}

    def find(x):
        parent.setdefault(x, x)
        while parent[x] != x:
            parent[x] = parent[parent[x]]
            x = parent[x]
        return x

    def union(a, b):
        ra, rb = find(a), find(b)
        if ra != rb:
            parent[max(ra, rb)] = min(ra, rb)

    for i in ids:
        find(i)
    for i, edges in sorted(edges_by_id.items()):
        if i in sub:
            continue  # a subsidiary keeps to itself
        for e in edges or []:
            n = e.get("node") or {}
            if e.get("relationType") in LINKS and n.get("type") == "ANIME" and n.get("format") in SERIES_FORMATS:
                if n["id"] in sub:
                    continue
                union(i, n["id"])
    for g in ex["merge"]:
        for x in g[1:]:
            union(g[0], x)
    return {i: find(i) for i in ids}
