"""The AniEx heat index, computed in the background (docs/v3-futures.md §3, step 2: it changes nothing in the game).

    uv run engine/heat_index.py [--day YYYY-MM-DD] [--report]

For each trading day D whose snapshots exist on both sides (the collector's run on D and on D+1, about 06:00
each), every show the collector tracks gets one row per market, from what was observed live, never from the
rebuilt series:

- us: the game's own attention for D (engine/economy.py `attention`: in-progress / 7, list adds, trending, score);
- cn: Bangumi, the change from D's to D+1's snapshot of the people who marked the show 在看, 看过, 搁置 or 抛弃
  (candidate), with 想看 and the episode comments kept for the calibration;
- jp: Annict, the change of the distinct (account, episode) pairs among the episodes' records (`viewerPairs`,
  collect.py `Viewers`: Annict lets an account record an episode again, so the raw `recordsCount` can be
  inflated by one account); while a work's pairs are not synced yet, the change of the summed `recordsCount`
  (4% higher on the busiest episodes). `watchersCount` is kept for the calibration.

A change is scaled to 24 hours by the two snapshots' fetch times (a manual re-run overwrites a day's snapshot).
Checks per market and show (§3.2): missing (a snapshot absent), negative (a cumulative count went down: a source
rollback; the candidates only grow otherwise),
spike (over SPIKE x the median of the show's previous 28 days, with 7 or more of them); a negative or spiking
value is replaced by that median, a missing one by the mean of the last 7 days, and the row says so.

index = sum over markets of weight x heat / platform size x SCALE, platform size = the mean daily total of the
market over the stored days (up to 365). Both SCALE and the platform rule are provisional until the switch
(§3.5). Rows are written once and never updated (INSERT OR IGNORE): the published-and-frozen rule, rehearsed.

Writes data/derived/heat_index.db (heat_raw: every measured change; heat_index: the checked values and the index).
--report compares a day with that day's inputs file (the heat the game uses now).
"""

from __future__ import annotations

import argparse
import json
import math
import os
import sqlite3
import statistics as st
import sys
from datetime import date, datetime, timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from engine import economy as E  # noqa: E402

VERSION = "shadow-2"
WEIGHTS = {"cn": 0.5, "us": 0.3, "jp": 0.2}     # server/game.toml [revenue] market weights
CANDIDATE = {"cn": "engaged", "jp": "viewers", "us": "attention"}
FALLBACK = {"jp": "records"}   # until a work's viewerPairs are synced (collect.py --annict-viewers)
SPIKE = 10.0
SCALE = 10000.0
OUT = Path(os.environ.get("ANISTOCK_DERIVED_DIR", ROOT / "data" / "derived")) / "heat_index.db"

SCHEMA = """
CREATE TABLE IF NOT EXISTS heat_raw (
    work_id   INTEGER NOT NULL,      -- AniList id
    day       TEXT NOT NULL,         -- trading day
    market    TEXT NOT NULL,
    metric    TEXT NOT NULL,
    value     REAL,                  -- per 24 hours; NULL when a snapshot is missing
    span_h    REAL,                  -- hours between the two snapshots
    PRIMARY KEY (work_id, day, market, metric)
);
CREATE TABLE IF NOT EXISTS heat_index (
    work_id     INTEGER NOT NULL,
    day         TEXT NOT NULL,
    us          REAL, cn REAL, jp REAL,   -- checked heat per market (NULL: the show has no source there)
    platform_us REAL, platform_cn REAL, platform_jp REAL,
    idx         REAL,
    flags       TEXT NOT NULL,            -- JSON: market -> missing | negative | spike (replaced) | filled
    version     TEXT NOT NULL,
    computed_at TEXT NOT NULL,
    PRIMARY KEY (work_id, day)
);
"""


def collector_path() -> Path:
    return Path(os.environ.get("ANISTOCK_DB", ROOT / "data" / "anistock.db"))


def snapshots(db: sqlite3.Connection, source: str, kind: str, day: str) -> dict[str, tuple[str, dict]]:
    return {ext: (fetched, json.loads(data)) for ext, fetched, data in db.execute(
        "SELECT external_id, fetched_at, data FROM snapshots WHERE source=? AND kind=? AND day=?", (source, kind, day))}


def hours(a: str, b: str) -> float:
    return (datetime.fromisoformat(b) - datetime.fromisoformat(a)).total_seconds() / 3600


def bgm_metrics(d: dict) -> dict[str, float]:
    c = d.get("collection") or {}
    # everyone who has marked the show as watching, watched, on hold or dropped: moves between these cancel, so
    # the change is people who started it (a net count like doing + collect falls when someone drops the show)
    return {"engaged": sum(c.get(k, 0) for k in ("doing", "collect", "on_hold", "dropped")), "wish": c.get("wish", 0)}


def bgm_comments(d: dict) -> float:
    return float(sum(e.get("comment", 0) for e in d.get("data", [])))


def annict_metrics(d: dict) -> dict[str, float]:
    return {"watchers": d.get("watchersCount") or 0,
            "records": sum((e.get("recordsCount") or 0) for e in d.get("episodes") or []),
            "viewers": d.get("viewerPairs")}


def raw_day(col: sqlite3.Connection, day: str) -> dict[int, dict[str, dict[str, tuple[float | None, float | None]]]]:
    """work -> market -> metric -> (change per 24 h, span hours) for trading day `day`."""
    nxt = (date.fromisoformat(day) + timedelta(days=1)).isoformat()
    from server.config import load
    cfg = load()
    p = cfg.params
    shows, _ = E.load_shows(col_path := collector_path(), p, date.fromisoformat(p["window"]["start"]), date.fromisoformat(day))
    matches = E.effective_matches(col_path)
    s0, s1 = snapshots(col, "bgm", "subject", day), snapshots(col, "bgm", "subject", nxt)
    e0, e1 = snapshots(col, "bgm", "episodes", day), snapshots(col, "bgm", "episodes", nxt)
    a0, a1 = snapshots(col, "annict", "work", day), snapshots(col, "annict", "work", nxt)
    out: dict[int, dict] = {}
    d = date.fromisoformat(day)
    for s in shows:
        if s.adult or s.promo:
            continue
        m: dict[str, dict] = {"us": {"attention": (E.attention(s, d, p["revenue"]) if d >= s.days[0] else None, 24.0)}}
        r = matches.get(str(s.id)) or matches.get(s.id) or {}
        if r.get("bgm"):
            k = str(r["bgm"])
            m["cn"] = {}
            if k in s0 and k in s1:
                h = hours(s0[k][0], s1[k][0])
                x0, x1 = bgm_metrics(s0[k][1]), bgm_metrics(s1[k][1])
                for name in x0:
                    m["cn"][name] = ((x1[name] - x0[name]) * 24 / h if h > 0 else None, h)
            else:
                m["cn"] = {"engaged": (None, None), "wish": (None, None)}
            if k in e0 and k in e1 and e0[k][1].get("offset") == e1[k][1].get("offset"):
                h = hours(e0[k][0], e1[k][0])
                m["cn"]["comments"] = ((bgm_comments(e1[k][1]) - bgm_comments(e0[k][1])) * 24 / h if h > 0 else None, h)
        if r.get("annict"):
            k = str(r["annict"])
            m["jp"] = {}
            if k in a0 and k in a1:
                h = hours(a0[k][0], a1[k][0])
                x0, x1 = annict_metrics(a0[k][1]), annict_metrics(a1[k][1])
                for name in x0:
                    known = h > 0 and x0[name] is not None and x1[name] is not None
                    m["jp"][name] = ((x1[name] - x0[name]) * 24 / h if known else None, h)
            else:
                m["jp"] = {"watchers": (None, None), "records": (None, None), "viewers": (None, None)}
        out[s.id] = m
    return out


def history(db: sqlite3.Connection, work: int, market: str, day: str, n: int) -> list[float]:
    col = {"us": "us", "cn": "cn", "jp": "jp"}[market]
    return [r[0] for r in db.execute(
        f"SELECT {col} FROM heat_index WHERE work_id=? AND day<? AND {col} IS NOT NULL ORDER BY day DESC LIMIT ?",
        (work, day, n))]


def compute(day: str) -> int:
    col = sqlite3.connect(f"file:{collector_path()}?mode=ro", uri=True)
    nxt = (date.fromisoformat(day) + timedelta(days=1)).isoformat()
    have = col.execute("SELECT COUNT(*) FROM snapshots WHERE source='bgm' AND kind='subject' AND day=?", (nxt,)).fetchone()[0]
    if not have:
        print(f"{day}: no snapshots for {nxt} yet")
        return 0
    OUT.parent.mkdir(parents=True, exist_ok=True)
    db = sqlite3.connect(OUT)
    db.executescript(SCHEMA)
    if db.execute("SELECT COUNT(*) FROM heat_index WHERE day=?", (day,)).fetchone()[0]:
        print(f"{day}: already computed (frozen)")
        return 0
    raw = raw_day(col, day)
    now = datetime.now().astimezone().isoformat(timespec="seconds")
    rows, flags_all = [], {}
    for work, markets in raw.items():
        vals, flags = {}, {}
        for market, metrics in markets.items():
            for metric, (v, h) in metrics.items():
                db.execute("INSERT OR IGNORE INTO heat_raw VALUES (?,?,?,?,?,?)", (work, day, market, metric, v, h))
            v = metrics.get(CANDIDATE[market], (None, None))[0]
            if v is None and market in FALLBACK:
                v = metrics.get(FALLBACK[market], (None, None))[0]
            past = history(db, work, market, day, 28)
            med = st.median(past) if len(past) >= 7 else None
            if v is None:
                recent = past[:7]
                v = st.fmean(recent) if recent else None
                flags[market] = "missing" if v is None else "filled"
            elif v < 0:
                flags[market] = "negative"
                v = med if med is not None else 0.0
            elif med is not None and med > 0 and v > SPIKE * med:
                flags[market] = "spike"
                v = med
            vals[market] = v
        rows.append((work, vals, flags))
        flags_all[work] = flags
    # platform size: the mean daily total per market over the stored days and this one (up to 365 days)
    platform = {}
    for market in WEIGHTS:
        totals = [r[0] for r in db.execute(
            f"SELECT SUM({market}) FROM heat_index WHERE {market} IS NOT NULL AND day<? GROUP BY day ORDER BY day DESC LIMIT 364",
            (day,))]
        today_total = sum(v.get(market) or 0.0 for _, v, _ in rows)
        totals.append(today_total)
        platform[market] = st.fmean(totals) if totals and st.fmean(totals) > 0 else None
    n = 0
    for work, vals, flags in rows:
        idx = sum(WEIGHTS[m] * (vals[m] or 0.0) / platform[m] * SCALE for m in WEIGHTS if vals.get(m) is not None and platform[m])
        db.execute("INSERT OR IGNORE INTO heat_index VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
                   (work, day, vals.get("us"), vals.get("cn"), vals.get("jp"),
                    platform["us"], platform["cn"], platform["jp"], idx, json.dumps(flags), VERSION, now))
        n += 1
    db.commit()
    bad = {k: sum(1 for f in flags_all.values() if k in f.values()) for k in ("missing", "filled", "negative", "spike")}
    print(f"{day}: {n} shows; flags " + " ".join(f"{k}={v}" for k, v in bad.items()))
    return n


def pending_days() -> list[str]:
    col = sqlite3.connect(f"file:{collector_path()}?mode=ro", uri=True)
    days = [r[0] for r in col.execute("SELECT DISTINCT day FROM snapshots WHERE source='bgm' AND kind='subject' ORDER BY day")]
    done = set()
    if OUT.exists():
        done = {r[0] for r in sqlite3.connect(OUT).execute("SELECT DISTINCT day FROM heat_index")} if \
            sqlite3.connect(OUT).execute("SELECT name FROM sqlite_master WHERE name='heat_index'").fetchone() else set()
    return [d for d, nxt in zip(days, days[1:]) if d not in done and (date.fromisoformat(nxt) - date.fromisoformat(d)).days == 1]


def report(day: str) -> None:
    db = sqlite3.connect(f"file:{OUT}?mode=ro", uri=True)
    inputs = Path(os.environ.get("ANISTOCK_INPUTS_DIR", ROOT / "data" / "derived" / "inputs")) / f"{day}.json"
    att = {x["id"]: x["att"] for x in json.loads(inputs.read_text())["shows"]} if inputs.exists() else {}
    rows = db.execute("SELECT work_id, us, cn, jp, idx, flags FROM heat_index WHERE day=?", (day,)).fetchall()
    print(f"{day}: {len(rows)} shows in the index; inputs file {'found' if att else 'missing'}")
    for i, market in enumerate(("us", "cn", "jp"), start=1):
        pairs = [(r[i], att[r[0]][market]) for r in rows if r[i] is not None and r[0] in att and r[i] > 0 and att[r[0]][market] > 0]
        if len(pairs) >= 5:
            xs, ys = [math.log(a) for a, _ in pairs], [math.log(b) for _, b in pairs]
            mx, my = st.fmean(xs), st.fmean(ys)
            cov = sum((x - mx) * (y - my) for x, y in zip(xs, ys))
            r = cov / math.sqrt(sum((x - mx) ** 2 for x in xs) * sum((y - my) ** 2 for y in ys))
            print(f"  {market}: {len(pairs)} shows with both; log correlation with the game's heat {r:+.2f}")
        else:
            print(f"  {market}: {len(pairs)} shows with both")
    idx = sorted(r[4] for r in rows if r[4])
    if idx:
        print(f"  index: median {st.median(idx):.1f}, p90 {idx[int(len(idx) * .9)]:.1f}, max {idx[-1]:.1f}")
    flagged = [(r[0], r[5]) for r in rows if r[5] != "{}"]
    print(f"  flagged {len(flagged)}: " + ", ".join(f"{w} {f}" for w, f in flagged[:10]))


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--day", help="one trading day (default: every day not computed yet)")
    ap.add_argument("--report", action="store_true")
    args = ap.parse_args()
    days = [args.day] if args.day else pending_days()
    for d in days:
        compute(d)
    if not days:
        print("nothing new to compute (every day with next-day snapshots is done; values are frozen)")
    if args.report:
        latest = sqlite3.connect(OUT).execute("SELECT MAX(day) FROM heat_index").fetchone()[0] if OUT.exists() else None
        for d in days or ([latest] if latest else []):
            report(d)


if __name__ == "__main__":
    main()
