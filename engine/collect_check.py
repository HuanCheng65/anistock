"""Did today's collection work? (docs/design/futures.md §6 step 1: the heat index is only as good as the data under it.)

    uv run engine/collect_check.py [--day YYYY-MM-DD]

Checks, for the collector's run on DAY (default: the day of its latest run, Asia/Shanghai):
- every source (bgm, anilist, annict) has a run that day and its last one ended `ok`;
- each snapshot kind covers at least 90% of the works it covered the day before;
- AniList trend rows for the previous UTC day cover at least 90% of the media of the day before that;
- the heat index (data/derived/heat_index.db), once computed for the previous trading day, has a value for every
  market of at least 90% of its shows (missing values are filled from the last days, but many at once mean a
  source failed).

Prints one line per check, writes data/derived/collect_status.json, and exits 1 when a check fails, so the
collector's systemd unit shows as failed. With ANISTOCK_ALERT_WEBHOOK set, a failure is also POSTed there as JSON
{"text": ...} (any chat bot or mail relay that takes a JSON text field).
"""

from __future__ import annotations

import argparse
import json
import os
import sqlite3
import sys
from datetime import date, datetime, timedelta, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TZ = timezone(timedelta(hours=8))
MIN_COVER = 0.9


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--day", help="the collection day to check (default: the day of the latest run)")
    args = ap.parse_args()
    col = sqlite3.connect(f"file:{os.environ.get('ANISTOCK_DB', ROOT / 'data' / 'anistock.db')}?mode=ro", uri=True)
    # by hand before the 06:00 run, "today" has nothing yet: check the latest collection instead
    day = args.day or (col.execute("SELECT MAX(substr(started_at, 1, 10)) FROM runs").fetchone()[0]
                       or datetime.now(TZ).date().isoformat())
    prev = (date.fromisoformat(day) - timedelta(days=1)).isoformat()
    checks: list[tuple[bool, str]] = []

    for source in ("bgm", "anilist", "annict"):
        r = col.execute("SELECT status, detail FROM runs WHERE source=? AND substr(started_at,1,10)=? ORDER BY id DESC LIMIT 1",
                        (source, day)).fetchone()
        if r is None:
            checks.append((False, f"{source}: no run on {day}"))
        else:
            checks.append((r[0] in ("ok", "skipped"), f"{source}: last run {r[0]} ({(r[1] or '')[:80]})"))

    for source, kind in col.execute("SELECT DISTINCT source, kind FROM snapshots WHERE day=?", (prev,)).fetchall():
        if source == "filmarks":
            continue    # its season lists turn over each quarter and its work pages are weekly (collect.py)
        a = col.execute("SELECT COUNT(*) FROM snapshots WHERE source=? AND kind=? AND day=?", (source, kind, prev)).fetchone()[0]
        b = col.execute("SELECT COUNT(*) FROM snapshots WHERE source=? AND kind=? AND day=?", (source, kind, day)).fetchone()[0]
        checks.append((b >= MIN_COVER * a, f"{source} {kind}: {b} snapshots ({a} the day before)"))

    t1 = (date.fromisoformat(day) - timedelta(days=1)).isoformat()
    t0 = (date.fromisoformat(day) - timedelta(days=2)).isoformat()
    a = col.execute("SELECT COUNT(DISTINCT media_id) FROM anilist_trends WHERE date=?", (t0,)).fetchone()[0]
    b = col.execute("SELECT COUNT(DISTINCT media_id) FROM anilist_trends WHERE date=?", (t1,)).fetchone()[0]
    checks.append((b >= MIN_COVER * a, f"anilist trends {t1}: {b} media ({a} on {t0})"))

    hi = Path(os.environ.get("ANISTOCK_DERIVED_DIR", ROOT / "data" / "derived")) / "heat_index.db"
    if hi.exists():
        db = sqlite3.connect(f"file:{hi}?mode=ro", uri=True)
        rows = db.execute("SELECT flags FROM heat_index WHERE day=?", (prev,)).fetchall()
        if rows:
            missing = sum(1 for (f,) in rows if "missing" in f or "filled" in f)
            checks.append((missing <= (1 - MIN_COVER) * len(rows), f"heat index {prev}: {len(rows)} shows, {missing} with a missing market"))
        else:
            checks.append((True, f"heat index {prev}: not computed yet"))

    ok = all(c for c, _ in checks)
    for c, line in checks:
        print(("ok    " if c else "FAIL  ") + line)
    status = {"day": day, "ok": ok, "checks": [{"ok": c, "text": t} for c, t in checks],
              "at": datetime.now(TZ).isoformat(timespec="seconds")}
    out = Path(os.environ.get("ANISTOCK_DERIVED_DIR", ROOT / "data" / "derived")) / "collect_status.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(status, ensure_ascii=False, indent=1), encoding="utf-8")
    hook = os.environ.get("ANISTOCK_ALERT_WEBHOOK")
    if not ok and hook:
        import httpx
        text = "安易采集检查未通过（" + day + "）：\n" + "\n".join(t for c, t in checks if not c)
        try:
            httpx.post(hook, json={"text": text}, timeout=10)
        except Exception as e:  # the check's own result stands; the alert is best effort
            print(f"alert webhook failed: {e}", file=sys.stderr)
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
