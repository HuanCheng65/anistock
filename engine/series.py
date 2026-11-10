"""Series grouping (docs/design/series.md §3.1): one company per series.

Default rule: works connected by SEQUEL / PREQUEL relations among TV, TV_SHORT, ONA and MOVIE entries are one series
(through entries the game does not track, too: a first season outside the data still links its second and third).
An OVA or SPECIAL joins the series it hangs from (SEQUEL, PREQUEL, PARENT or SIDE_STORY to a work of exactly one
series; recaps, AniList SUMMARY, never). Other side stories, spin-offs and alternatives are not joined. The exception
table `sim/series.json` (in git, kept by hand like sim/matches.json) can:

    "subsidiary": {"<AniList id>": "<parent id>", ...}   list a work as its own company (its audience does not move
                                                        with the main line); the parent is shown on its profile
    "merge":      [["<id>", "<id>", ...], ...]          join works AniList does not link (a renamed reboot)
    "cut":        [["<id>", "<id>"], ...]               a SEQUEL/PREQUEL relation not followed, either way (a line cut
                                                        from a franchise of decades; a remake made decades later, cut
                                                        from the original)
    "names":      {"<id>": "<name>", ...}               the series' name, by any of its works (default: its first TV
                                                        or ONA work's Chinese title, engine/daily_inputs.py
                                                        `series_titles`)

A series' key is the smallest AniList id in it (deterministic, independent of what is listed). Pure functions; the
relations come from the collector's AniList 'relations' snapshots (engine/daily_inputs.py `load_relations`).
"""

from __future__ import annotations

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SERIES_FORMATS = {"TV", "TV_SHORT", "ONA", "MOVIE"}
LINKS = {"SEQUEL", "PREQUEL"}
ATTACH_FORMATS = {"OVA", "SPECIAL"}
ATTACH_LINKS = {"SEQUEL", "PREQUEL", "PARENT", "SIDE_STORY"}


def load_exceptions(path: Path | None = None) -> dict:
    f = path or ROOT / "sim" / "series.json"
    raw = json.loads(f.read_text(encoding="utf-8")) if f.exists() else {}
    return {"subsidiary": {int(k): int(v) for k, v in raw.get("subsidiary", {}).items()},
            "merge": [[int(x) for x in g] for g in raw.get("merge", [])],
            "cut": [(int(a), int(b)) for a, b in raw.get("cut", [])],
            "names": {int(k): v for k, v in raw.get("names", {}).items()}}


def _cuts(ex: dict) -> set[frozenset]:
    return {frozenset(p) for p in ex.get("cut", [])}


def edge_kind(e: dict, from_attached: bool = False) -> str | None:
    """From a TV/TV_SHORT/ONA/film: 'series' for a SEQUEL/PREQUEL relation to another, 'attach' for an ATTACH_LINKS
    relation to an OVA or SPECIAL. From an OVA or SPECIAL (`from_attached`): 'hang' for an ATTACH_LINKS relation to a
    TV/TV_SHORT/ONA/film. Else None."""
    n = e.get("node") or {}
    rt = e.get("relationType")
    if n.get("type") != "ANIME":
        return None
    if from_attached:
        return "hang" if rt in ATTACH_LINKS and n.get("format") in SERIES_FORMATS else None
    if rt in LINKS and n.get("format") in SERIES_FORMATS:
        return "series"
    if rt in ATTACH_LINKS and n.get("format") in ATTACH_FORMATS:
        return "attach"
    return None


def series_keys(edges_by_id: dict[int, list], ids, exceptions: dict | None = None,
                formats: dict[int, str] | None = None) -> dict[int, int]:
    """{id: series key} for every id in `ids`. edges_by_id: each work's AniList relation edges (latest snapshot);
    formats: each id's own AniList format, where known (an OVA or SPECIAL in `ids` joins the series it hangs from)."""
    ex = exceptions or {"subsidiary": {}, "merge": [], "cut": []}
    sub = ex["subsidiary"]
    cut = _cuts(ex)
    fmt = dict(formats or {})
    for es in edges_by_id.values():
        for e in es or []:
            n = e.get("node") or {}
            if n.get("id") is not None and n.get("format"):
                fmt.setdefault(n["id"], n["format"])
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

    def attached(i):
        return fmt.get(i) in ATTACH_FORMATS

    for i in ids:
        find(i)
    hangs: dict[int, set[int]] = {}     # an OVA/SPECIAL -> the series works it is related to
    for i, edges in sorted(edges_by_id.items()):
        if i in sub:
            continue  # a subsidiary keeps to itself
        for e in edges or []:
            n = e.get("node") or {}
            k = edge_kind(e, attached(i))
            if k is None or n["id"] in sub:
                continue
            if k == "series" and frozenset((i, n["id"])) not in cut:
                union(i, n["id"])
            elif k == "hang":
                hangs.setdefault(i, set()).add(n["id"])     # the OVA's own edge to its series
            elif k == "attach":
                hangs.setdefault(n["id"], set()).add(i)
    for g in ex["merge"]:
        for x in g[1:]:
            union(g[0], x)
    out = {}
    for i in ids:
        if attached(i) and i not in sub:
            roots = {find(x) for x in hangs.get(i, ())}
            out[i] = roots.pop() if len(roots) == 1 else find(i)   # hung from two series: neither
        else:
            out[i] = find(i)
    return out


def members(seeds, edges_of, exceptions: dict | None = None, limit: int = 200) -> set[int]:
    """The works of the seeds' series (SEQUEL/PREQUEL through TV/TV_SHORT/ONA/films, plus the OVAs and SPECIALs hanging
    from them, `series_keys`'s rule), walking the relations: edges_of(ids) -> {id: edges}, asked once per step for the
    works first met in it. The seeds themselves are included. `limit` stops a walk through a franchise of hundreds."""
    ex = exceptions or {"subsidiary": {}, "merge": [], "cut": []}
    sub, cut = ex["subsidiary"], _cuts(ex)
    merged = {x: g for g in ex["merge"] for x in g}
    seen = set(seeds)
    frontier = [s for s in seeds if s not in sub]
    while frontier and len(seen) < limit:
        got = edges_of(frontier)
        nxt = []
        for x in frontier:
            for y in merged.get(x, ()):
                if y not in seen:
                    seen.add(y)
                    nxt.append(y)
            for e in got.get(x) or []:
                n = e.get("node") or {}
                k = edge_kind(e)
                if k is None or n["id"] in seen or n["id"] in sub or (k == "series" and frozenset((x, n["id"])) in cut):
                    continue
                seen.add(n["id"])
                if k == "series":
                    nxt.append(n["id"])     # an OVA's own relations are not walked: it joins, it does not link
        frontier = nxt
    return seen
