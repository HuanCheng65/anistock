"""The heat index's history from the backfills (docs/v3-futures.md §3.6), for calibration: never published, never
mixed with the live, frozen values of data/derived/heat_index.db.

    uv run sim/heat_history.py [--from 2026-01-01] [--until DAY] [--markets DIR]

Per AniList show and day, the three markets measured the way engine/heat_index.py measures them live:
- us: the game's attention (engine/economy.py `attention`) from the collector's AniList trends;
- cn: Bangumi, people who marked the show (done, doing, on hold, dropped), from the weekly archive dumps
  (sim/backfill_bgm_archive.py): each week's change spread evenly over the days between two dumps;
- jp: Annict records logged that day (sim/backfill_annict_records.py).

Writes heat_history(work_id, day, us, cn, jp) into data/derived/heat_backfill.db and prints the calibration:
per market, how the measure relates to the rebuilt series the game uses now (data/derived/markets.json, or
--markets DIR): the log correlation of weekly sums across shows and within shows over time, and their ratio;
the platform size (the mean daily total over the period); the index's spread with the provisional scale.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import sqlite3
import statistics as st
import sys
from bisect import bisect_right
from collections import defaultdict
from datetime import date, datetime, timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
from engine import economy as E  # noqa: E402
from engine.heat_index import SCALE, WEIGHTS  # noqa: E402

DB = Path(os.environ.get("ANISTOCK_DERIVED_DIR", ROOT / "data" / "derived")) / "heat_backfill.db"


def days_between(a: date, b: date):
    d = a
    while d <= b:
        yield d
        d += timedelta(days=1)


def bgm_daily(db: sqlite3.Connection, table: str = "bgm_weekly") -> dict[int, dict[str, float]]:
    """subject -> day -> people who started it that day (a week's change spread over its days)."""
    out: dict[int, dict[str, float]] = defaultdict(dict)
    rows = defaultdict(list)
    for sid, at, done, doing, hold, dropped in db.execute(
            f"SELECT subject_id, at, done, doing, on_hold, dropped FROM {table} ORDER BY subject_id, at"):
        local = (datetime.fromisoformat(at.replace("Z", "+00:00")) + timedelta(hours=8)).date()
        rows[sid].append((local, sum(x or 0 for x in (done, doing, hold, dropped))))
    for sid, pts in rows.items():
        for (d0, n0), (d1, n1) in zip(pts, pts[1:]):
            span = (d1 - d0).days
            if span <= 0:
                continue
            per = max(0.0, (n1 - n0) / span)    # a week's net fall is a source correction, not negative activity
            for d in days_between(d0, d1 - timedelta(days=1)):
                out[sid][d.isoformat()] = per
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--from", dest="start", default="2026-01-01")
    ap.add_argument("--until", default=None)
    ap.add_argument("--markets", default=None, help="directory of the markets.json to compare with")
    ap.add_argument("--archive", action="store_true",
                    help="cn from every anime subject of the archive (bgm_anime_weekly), with the title matches of "
                         "sim/match_archive.py for shows without one")
    args = ap.parse_args()
    from server.config import load
    p = load().params
    db = sqlite3.connect(DB)
    col = Path(os.environ.get("ANISTOCK_DB", ROOT / "data" / "anistock.db"))
    until = args.until or db.execute("SELECT MAX(day) FROM annict_daily").fetchone()[0]
    until = (date.fromisoformat(until) - timedelta(days=1)).isoformat()   # the last day may be partial
    start, end = date.fromisoformat(args.start), date.fromisoformat(until)
    shows, _ = E.load_shows(col, p, start, end)
    matches = E.effective_matches(col)
    table = "bgm_anime_weekly" if args.archive else "bgm_weekly"
    if args.archive:
        for aid, sid in json.loads((DB.parent / "archive_matches.json").read_text()).items():
            matches.setdefault(aid, {}).setdefault("bgm", sid)
    cn = bgm_daily(db, table)
    jp: dict[int, dict[str, int]] = defaultdict(dict)
    for aid, day, n in db.execute("SELECT annict_id, day, records FROM annict_daily"):
        jp[aid][day] = n
    jp_seen = {r[0] for r in db.execute("SELECT DISTINCT annict_id FROM annict_done")}
    cn_seen = {r[0] for r in db.execute(f"SELECT DISTINCT subject_id FROM {table}")}
    cn_last = max((max(v) for v in cn.values() if v), default="")   # after the last dump: not measured yet

    db.execute("CREATE TABLE IF NOT EXISTS heat_history (work_id INTEGER NOT NULL, day TEXT NOT NULL, us REAL, cn REAL, jp REAL, PRIMARY KEY (work_id, day))")
    db.execute("DELETE FROM heat_history")
    hist = []
    for s in shows:
        if s.adult or s.promo:
            continue
        r = matches.get(str(s.id)) or {}
        b = int(r["bgm"]) if r.get("bgm") else None
        a = int(r["annict"]) if r.get("annict") else None
        for d in days_between(max(start, s.days[0]), end):
            ds = d.isoformat()
            us = E.attention(s, d, p["revenue"])
            c = cn.get(b, {}).get(ds, 0.0 if b in cn_seen else None) if b and ds <= cn_last else None
            j = jp.get(a, {}).get(ds, 0 if a in jp_seen else None) if a else None
            hist.append((s.id, ds, us, c, j))
    with db:
        db.executemany("INSERT INTO heat_history VALUES (?,?,?,?,?)", hist)
    print(f"{len(hist)} show-days, {start}..{end}")

    # ---- calibration against the rebuilt series
    mpath = Path(args.markets or ROOT / "data" / "derived") / "markets.json"
    series = json.loads(mpath.read_text())["series"]
    for k, market in ((3, "cn"), (4, "jp")):
        weekly_new, weekly_old = defaultdict(float), defaultdict(float)
        for wid, ds, *v in hist:
            x = v[k - 2]
            old = series.get(str(wid), {}).get(ds, {}).get(market)
            if x is None or old is None:
                continue
            wk = (wid, date.fromisoformat(ds).isocalendar()[:2])
            weekly_new[wk] += x
            weekly_old[wk] += old
        pairs = [(weekly_new[w], weekly_old[w]) for w in weekly_new if weekly_new[w] > 0 and weekly_old[w] > 0]
        if len(pairs) < 10:
            print(f"{market}: {len(pairs)} show-weeks with both")
            continue
        lx, ly = [math.log(a) for a, _ in pairs], [math.log(b) for _, b in pairs]

        def corr(xs, ys):
            mx, my = st.fmean(xs), st.fmean(ys)
            return sum((x - mx) * (y - my) for x, y in zip(xs, ys)) / math.sqrt(
                sum((x - mx) ** 2 for x in xs) * sum((y - my) ** 2 for y in ys))
        # within shows: demean each show's weeks (does the measure track the same rises and falls?)
        by = defaultdict(list)
        for (w, _), (a, b) in zip([(w, 0) for w in weekly_new if weekly_new[w] > 0 and weekly_old[w] > 0], pairs):
            by[w[0]].append((math.log(a), math.log(b)))
        wx, wy = [], []
        for v in by.values():
            if len(v) >= 4:
                ma, mb = st.fmean(a for a, _ in v), st.fmean(b for _, b in v)
                wx += [a - ma for a, _ in v]
                wy += [b - mb for _, b in v]
        ratio = st.median(a / b for a, b in pairs)
        print(f"{market}: {len(pairs)} show-weeks; log correlation across {corr(lx, ly):+.2f}, within shows "
              f"{corr(wx, wy) if len(wx) > 10 else float('nan'):+.2f}; median new/old {ratio:.3g}")

    # ---- platform sizes and the index's spread
    tot = defaultdict(lambda: defaultdict(float))
    for wid, ds, us, c, j in hist:
        for m, x in (("us", us), ("cn", c), ("jp", j)):
            if x:
                tot[m][ds] += x
    plat = {m: st.fmean(v.values()) for m, v in tot.items() if v}
    print("platform size (mean daily total): " + "  ".join(f"{m} {plat[m]:.0f}" for m in plat))
    last = end.isoformat()
    idx = sorted(sum(WEIGHTS[m] * (x or 0) / plat[m] * SCALE for m, x in (("us", us), ("cn", c), ("jp", j)) if m in plat)
                 for wid, ds, us, c, j in hist if ds == last)
    if idx:
        print(f"index on {last} (provisional scale {SCALE:g}): median {st.median(idx):.1f}  p90 {idx[int(len(idx) * .9)]:.1f}  max {idx[-1]:.1f}")


if __name__ == "__main__":
    main()
