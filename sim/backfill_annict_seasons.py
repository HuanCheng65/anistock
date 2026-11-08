"""Register past seasons' Annict works (with their episodes) in a collector database, for backfilling the heat
index's jp market over seasons the collector did not track (docs/design/futures.md §3.7; then
sim/backfill_annict_records.py counts their records). The snapshots are dated today, like a collection run.

    ANISTOCK_DB=data/fast/collector_b.db uv run python sim/backfill_annict_seasons.py 2025-autumn 2026-winter 2026-spring
"""

from __future__ import annotations

import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
import collect as C  # noqa: E402


def main() -> None:
    C.load_dotenv(ROOT / ".env")
    seasons = sys.argv[1:]
    store = C.Store(Path(os.environ.get("ANISTOCK_DB", ROOT / "data" / "anistock.db")))
    http = C.Http(min_interval=0.3, remaining_header="ratelimit-remaining", reset_header="ratelimit-reset",
                  headers={"Authorization": f"Bearer {os.environ['ANNICT_TOKEN'].strip()}"})
    found: dict[int, dict] = {}
    after = None
    while True:
        res = http.graphql(C.ANNICT, C.ANNICT_SEARCH, {"seasons": seasons, "after": after})["searchWorks"]
        stop = False
        for w in res["nodes"]:
            if w["watchersCount"] < C.MIN_ANNICT_WATCHERS:
                stop = True
                break
            found[w["annictId"]] = w
        if stop or not res["pageInfo"]["hasNextPage"]:
            break
        after = res["pageInfo"]["endCursor"]
    for wid, w in found.items():
        store.register("annict", wid, w["title"], w.get("malAnimeId"))
        eps, after = [], None
        while len(eps) < 100:
            r = http.graphql(C.ANNICT, C.ANNICT_EPISODES, {"id": wid, "after": after})["searchWorks"]["nodes"]
            if not r or not r[0]["episodes"]:
                break
            conn = r[0]["episodes"]
            eps.extend(conn["nodes"])
            if not conn["pageInfo"]["hasNextPage"]:
                break
            after = conn["pageInfo"]["endCursor"]
        store.snapshot("annict", "work", wid, {**w, "episodes": eps})
        store.commit()
    print(f"{len(found)} Annict works of {', '.join(seasons)} registered")


if __name__ == "__main__":
    main()
