"""热度快讯 fetcher: comment counts of today's new episodes on Bangumi, for the game server's NPC market.

    .venv/bin/python engine/flash_fetch.py --inbox data/events/inbox [--game-db data/game.db] [--now ISO]

The game server runs this once an hour while shows air (08:00-03:00). For every listed show whose episode
aired within the last 26 hours (Annict air times via sim/matches.json; else Bangumi's air date at 23:00
JST), it makes ONE request to the Bangumi API (the show's episode list), and writes one observation per show
as a JSON file into the inbox:

    {"company_id": 171018, "episode": 5, "comments": 312, "prev_comments": [250, 230, 280],
     "hours_since_air": 3.1, "fetched_at": "2026-11-05T02:05:00+08:00"}

`prev_comments` are the previous three episodes' current (practically final) counts. Files are written to a
temporary name and renamed, so the server never reads a half-written file. The server decides whether the
observation is news (backend/src/market/runtime.rs). Reads the game database read-only; network through
the standard proxy variables (HTTPS_PROXY/https_proxy), like collect.py.
"""

from __future__ import annotations

import argparse
import json
import os
import sqlite3
import sys
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path

import httpx

ROOT = Path(__file__).resolve().parent.parent
BGM = "https://api.bgm.tv"
USER_AGENT = "anistock-collector/0.1 (anime club market game)"
JST = timezone(timedelta(hours=9))
CST = timezone(timedelta(hours=8))


def listed_companies(game_db: Path) -> dict[int, str]:
    db = sqlite3.connect(f"file:{game_db}?mode=ro", uri=True)
    try:
        return {r[0]: r[1] for r in db.execute("SELECT id, title FROM companies WHERE status='listed'")}
    finally:
        db.close()


def air_times(matches: dict) -> dict[int, list[datetime]]:
    """AniList id -> episode air times (Annict), sorted."""
    path = ROOT / "data" / "derived" / "annict_airtimes.json"
    if not path.exists():
        return {}
    raw = json.loads(path.read_text(encoding="utf-8"))
    out = {}
    for aid, m in matches.items():
        eps = raw.get(str(m.get("annict"))) if m.get("annict") else None
        if eps:
            out[int(aid)] = sorted(datetime.fromisoformat(t.replace("Z", "+00:00")) for t in eps.values())
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--inbox", type=Path, required=True)
    ap.add_argument("--game-db", type=Path, default=Path(os.environ.get("ANISTOCK_GAME_DB", ROOT / "data" / "game.db")))
    ap.add_argument("--now", help="pretend it is this time (ISO, for testing)")
    ap.add_argument("--dry-run", action="store_true", help="print the observations instead of writing them")
    args = ap.parse_args()
    now = datetime.fromisoformat(args.now) if args.now else datetime.now(timezone.utc)

    m = json.loads((ROOT / "sim" / "matches.json").read_text(encoding="utf-8"))
    matches = {k: v for k, v in m.get("matches", {}).items()}
    for k, v in m.get("manual", {}).items():
        if "bgm" in v:
            matches.setdefault(k, {})["bgm"] = v["bgm"]
    companies = listed_companies(args.game_db)
    times = air_times(matches)

    # which shows have an episode today: Annict air times, else the air dates in the collector's latest
    # Bangumi episode list (no request needed to decide)
    known_dates: dict[int, set[str]] = {}
    totals: dict[int, int] = {}
    coll = Path(os.environ.get("ANISTOCK_DB", ROOT / "data" / "anistock.db"))
    if coll.exists():
        db = sqlite3.connect(f"file:{coll}?mode=ro", uri=True)
        for sid, data in db.execute("SELECT CAST(external_id AS INTEGER), data FROM snapshots "
                                    "WHERE source='bgm' AND kind='episodes' ORDER BY day"):
            d = json.loads(data)
            known_dates[sid] = {e.get("airdate") for e in d.get("data", [])}
            totals[sid] = d.get("total", 0)
        db.close()
    jst_today = now.astimezone(JST).date()
    days = {jst_today.isoformat(), (jst_today - timedelta(days=1)).isoformat()}

    client = httpx.Client(headers={"User-Agent": USER_AGENT}, timeout=httpx.Timeout(30.0, connect=15.0),
                          trust_env=True, follow_redirects=True)
    written = 0
    for cid, title in sorted(companies.items()):
        sid = (matches.get(str(cid)) or {}).get("bgm")
        if not sid:
            continue
        recent = [t for t in times.get(cid, []) if timedelta(0) <= now - t <= timedelta(hours=26)]
        if not recent and not (known_dates.get(sid, set()) & days):
            continue
        try:
            # one request per show: the latest 100 episodes
            r = client.get(f"{BGM}/v0/episodes",
                           params={"subject_id": sid, "type": 0, "limit": 100, "offset": max(0, totals.get(sid, 0) - 100)})
            r.raise_for_status()
            eps = sorted(r.json().get("data", []), key=lambda e: e.get("sort", 0))
        except (httpx.HTTPError, ValueError) as e:
            print(f"{cid} {title}: {e}", file=sys.stderr)
            continue
        time.sleep(0.5)
        aired_at = recent[-1] if recent else None
        if aired_at is None:
            for e in reversed(eps):
                d = e.get("airdate") or ""
                if d in (jst_today.isoformat(), (jst_today - timedelta(days=1)).isoformat()):
                    at = datetime.fromisoformat(d).replace(hour=23, tzinfo=JST)
                    if timedelta(0) <= now - at <= timedelta(hours=26):
                        aired_at = at
                    break
        if aired_at is None:
            continue
        # the episode aired on that (JST) date
        day = aired_at.astimezone(JST).date().isoformat()
        idx = next((i for i in range(len(eps) - 1, -1, -1) if eps[i].get("airdate") == day), None)
        if idx is None or idx == 0:
            continue
        ep = eps[idx]
        obs = {
            "company_id": cid,
            "episode": ep.get("sort"),
            "comments": int(ep.get("comment", 0)),
            "prev_comments": [int(x.get("comment", 0)) for x in eps[max(0, idx - 3):idx]],
            "hours_since_air": round((now - aired_at).total_seconds() / 3600, 2),
            "fetched_at": now.astimezone(CST).isoformat(timespec="seconds"),
        }
        if args.dry_run:
            print(json.dumps(obs, ensure_ascii=False))
            continue
        args.inbox.mkdir(parents=True, exist_ok=True)
        name = f"{now.astimezone(CST):%Y%m%d-%H%M}-{cid}.json"
        tmp = args.inbox / (name + ".tmp")
        tmp.write_text(json.dumps(obs, ensure_ascii=False), encoding="utf-8")
        tmp.replace(args.inbox / name)
        written += 1
    print(f"{written} observations")


if __name__ == "__main__":
    main()
