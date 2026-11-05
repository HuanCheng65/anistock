"""Fetch Bangumi episode comment timestamps for the hourly 热度快讯 curve (sim/fit_valuation.py --comments).

    .venv/bin/python sim/fetch_comments.py OUT.json [--from 2026-07-01 --to 2026-09-30] [--max 160]

Picks episodes (not a show's first) that aired in the window with at least 20 comments, spread over the
comment-count range, and fetches each one's comments from next.bgm.tv (one request per episode, 1 s apart).
Air time: Annict's time for that day when the show is matched, else 23:00 JST on Bangumi's air date. Also
records the previous three episodes' current comment counts. Reads the collector DB ($ANISTOCK_DB) read-only.
"""

from __future__ import annotations

import argparse
import json
import os
import sqlite3
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path

import httpx

ROOT = Path(__file__).resolve().parent.parent
JST = timezone(timedelta(hours=9))
UA = "anistock-collector/0.1 (anime club market game)"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out", type=Path)
    ap.add_argument("--from", dest="start", default="2026-07-01")
    ap.add_argument("--to", dest="end", default="2026-09-30")
    ap.add_argument("--max", type=int, default=160)
    a = ap.parse_args()
    coll = Path(os.environ.get("ANISTOCK_DB", ROOT / "data" / "anistock.db"))
    db = sqlite3.connect(f"file:{coll}?mode=ro", uri=True)
    m = json.loads((ROOT / "sim" / "matches.json").read_text(encoding="utf-8"))["matches"]
    annict_of_bgm = {v["bgm"]: v.get("annict") for v in m.values() if v.get("bgm")}
    air = json.loads((ROOT / "data" / "derived" / "annict_airtimes.json").read_text(encoding="utf-8"))
    cands = []
    for sid, data in db.execute("SELECT CAST(external_id AS INTEGER), data FROM snapshots "
                                "WHERE source='bgm' AND kind='episodes' ORDER BY day"):
        eps = sorted((e for e in json.loads(data).get("data", []) if e.get("type", 0) == 0), key=lambda e: e["sort"])
        for i, e in enumerate(eps):
            if i == 0 or not (a.start <= (e.get("airdate") or "") <= a.end) or (e.get("comment") or 0) < 20:
                continue
            cands.append((e["comment"], e["id"], sid, e["airdate"], [x.get("comment", 0) for x in eps[max(0, i - 3):i]]))
    cands = list({c[1]: c for c in cands}.values())
    cands.sort()
    n = min(a.max, len(cands))
    picks = [cands[int(k * (len(cands) - 1) / max(1, n - 1))] for k in range(n)]
    out = json.loads(a.out.read_text()) if a.out.exists() else {}
    with httpx.Client(headers={"User-Agent": UA}, timeout=30, trust_env=True) as c:
        for count, eid, sid, airdate, prev in picks:
            if str(eid) in out:
                continue
            # air time: Annict's time on that JST day, else 23:00 JST
            at = datetime.fromisoformat(airdate).replace(hour=23, tzinfo=JST)
            an = annict_of_bgm.get(sid)
            if an and str(an) in air:
                for t in air[str(an)].values():
                    tt = datetime.fromisoformat(t.replace("Z", "+00:00"))
                    if tt.astimezone(JST).date().isoformat() == airdate:
                        at = tt
                        break
            r = c.get(f"https://next.bgm.tv/p1/episodes/{eid}/comments")
            time.sleep(1.0)
            if r.status_code != 200:
                print(f"ep {eid}: HTTP {r.status_code}")
                continue
            js = r.json()
            times = [x["createdAt"] for x in js] + [y["createdAt"] for x in js for y in x.get("replies", [])]
            out[str(eid)] = {"subject": sid, "air": int(at.timestamp()), "times": sorted(times), "prev_final": prev,
                             "final": count}
            print(f"ep {eid}: {len(times)} comments")
            a.out.write_text(json.dumps(out))
    print(f"{len(out)} episodes")


if __name__ == "__main__":
    main()
