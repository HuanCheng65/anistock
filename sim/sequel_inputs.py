"""Replay inputs with the sequel facts (docs/design/series.md §5): copy a daily-inputs directory and rewrite only
`prequel`, `sequel` and `relations_known` from relations as they were known on each day.

    python sim/sequel_inputs.py IN_DIR OUT_DIR --collector DB [--collector DB ...]

The replay's inputs were built before the collector kept AniList relations (it has since 2026-11-06), so every show
reads "relations unknown" and no sequel is ever announced. Here each show's relations are its latest snapshot in the
collector databases, with every SEQUEL entry held back until its announcement: the entry's first AniList trend day,
the announcement date the hazard model was fitted on (sim/fit_sequel.py, sim/tail_history.py `announced`). Those days
are read from AniList once (one request per 40 entries) and cached in data/derived/sequel_first_trend.json. Everything
else in the inputs is unchanged, so a replay on OUT_DIR differs from one on IN_DIR only by the sequel facts.
"""

from __future__ import annotations

import argparse
import json
import shutil
import sqlite3
import sys
import urllib.request
from datetime import date, datetime, timedelta, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from engine.daily_inputs import SERIES_FORMATS, series_facts  # noqa: E402

JST = timezone(timedelta(hours=9))
EPOCH = date(2000, 1, 1)


def latest_relations(dbs: list[str]) -> dict[int, list]:
    out: dict[int, tuple[str, list]] = {}
    for p in dbs:
        db = sqlite3.connect(f"file:{p}?mode=ro", uri=True)
        for mid, d, data in db.execute(
                "SELECT external_id, day, data FROM snapshots WHERE source='anilist' AND kind='relations'"):
            if int(mid) not in out or d > out[int(mid)][0]:
                out[int(mid)] = (d, json.loads(data).get("edges") or [])
        db.close()
    return {k: v[1] for k, v in out.items()}


def is_sequel(e: dict) -> bool:
    return e["relationType"] == "SEQUEL" and e["node"].get("format") in SERIES_FORMATS


def first_trend(ids: list[int], cache: Path) -> dict[int, str | None]:
    have = {int(k): v for k, v in json.loads(cache.read_text())} if cache.exists() else {}
    todo = [i for i in ids if i not in have]
    for i in range(0, len(todo), 40):
        batch = todo[i:i + 40]
        q = "{" + " ".join(f"f{m}: Page(perPage:1){{ mediaTrends(mediaId:{m}, sort:DATE){{date}} }}" for m in batch) + "}"
        req = urllib.request.Request("https://graphql.anilist.co", data=json.dumps({"query": q}).encode(), headers={
            "Content-Type": "application/json", "Accept": "application/json", "User-Agent": "anistock-collector/0.1"})
        d = json.load(urllib.request.urlopen(req, timeout=60))["data"]
        for m in batch:
            r = d[f"f{m}"]["mediaTrends"]
            have[m] = datetime.fromtimestamp(r[0]["date"], JST).date().isoformat() if r else None
    cache.write_text(json.dumps(sorted(have.items())))
    return have


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("inputs")
    ap.add_argument("out")
    ap.add_argument("--collector", action="append", required=True)
    a = ap.parse_args()
    src, dst = Path(a.inputs), Path(a.out)
    dst.mkdir(parents=True, exist_ok=True)
    files = sorted(src.glob("????-??-??.json"))
    ids = sorted({s["id"] for f in files for s in json.loads(f.read_text())["shows"]})
    rel = latest_relations(a.collector)
    missing = [i for i in ids if i not in rel]
    if missing:
        sys.exit(f"no relations for {len(missing)} shows, e.g. {missing[:10]}")
    seq = sorted({e["node"]["id"] for i in ids for e in rel[i] if is_sequel(e)})
    ft = first_trend(seq, ROOT / "data" / "derived" / "sequel_first_trend.json")
    # the sequel's status is decided by its dates as of each day, not by today's status
    snaps: dict[int, list] = {}
    for i in ids:
        base = [e for e in rel[i] if not is_sequel(e)]
        held = []
        for e in rel[i]:
            if is_sequel(e):
                day = ft.get(e["node"]["id"])
                if day is None:
                    print(f"  {i} -> {e['node']['id']}: no trend day, never announced in the replay")
                    continue
                held.append((date.fromisoformat(day), {**e, "node": {**e["node"], "status": None}}))
        days = sorted({d for d, _ in held})
        snaps[i] = [(EPOCH, base)] + [(d, base + [e for dd, e in held if dd <= d]) for d in days]
        for d, e in held:
            print(f"  {i} -> {e['node']['id']} announced {d}")
    lags = json.loads((ROOT / "sim" / "calibration" / "sequel.json").read_text()).get("lag_days", [])
    n = 0
    for f in files:
        x = json.loads(f.read_text())
        day = date.fromisoformat(x["day"])
        for s in x["shows"]:
            s.update(series_facts(s["id"], snaps, day, lags))
            n += s["sequel"] is not None
        (dst / f.name).write_text(json.dumps(x, ensure_ascii=False))
    for f in src.iterdir():
        if f.is_file() and not (dst / f.name).exists():
            shutil.copy2(f, dst / f.name)
    print(f"{len(files)} days, {len(ids)} shows, {sum(1 for i in ids if any(is_sequel(e) for e in rel[i]))} with a "
          f"sequel entry; show-days with an announced sequel: {n}")


if __name__ == "__main__":
    main()
