"""Recompute the pre-launch history (the 'replay' rows) of a game database with the current model.

    python -m server.tools.rebuild_replay GAME_DB [--dry-run] [--force] [--inputs DIR] [--bin PATH] [--work DIR]

Operator steps (Chinese): docs/ops/rebuild-replay.md. Run it with the server STOPPED: the server keeps the
companies and their history in memory and would not see the change (and could write over it).

Before launch, server/seed.py filled each stock's history from the window's first day with a replay of the model
of that time, scaled so the last replay close is the opening price (source='replay' candles, plus company_days
revenue rows). After a model change that history no longer matches the live part. This tool:

1. seeds a scratch start world on the window's first day (`server.seed --until`, the same seed path, with the
   game's own demand reference `a_ref` copied in);
2. runs the current model's integrated replay from there to the day before launch (`anistock replay-history`:
   the valuation first replays `--warmup-days` of inputs before the window, so shows already on air start
   from what their observations up to then say, not from their pre-air prior;
   the real engine, the real daily close, the NPC market; each day's close reads only that day's inputs file
   from data/derived/inputs or $ANISTOCK_INPUTS_DIR, so nothing later leaks into earlier days);
3. per stock, scales the new price path (and the IPO price) so its last replay close equals the current last
   replay close, which is the opening reference price seed.py set and the first live day's previous close (the
   same rule seed.py used; the rewrite keeps it, so running the tool again gives the same result);
4. rewrites, in one transaction, only: the stock's source='replay' candles, its company_days before launch
   (revenue, by market, episodes aired, dividend per share: the revenue chart has no break at launch) and
   companies.ipo_price. Everything else (accounts, holdings, lots, orders, trades, ledger, valuation_joint,
   every row dated on or after launch, ...) is hashed before and after; any difference rolls the change back.

A backup (SQLite backup API) is written next to the database first: GAME_DB's name + '-before-rebuild-<time>.db'.
Stocks the new replay does not carry to the day before launch (not listed by then, delisted or merged in the
replay) keep their old rows and are listed in the report.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import tempfile
from datetime import date, datetime, timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent

# what the tool may change; every other table is compared whole before and after the rewrite
OWN = {"candles", "company_days", "companies"}


def holders(db: Path) -> list[tuple[int, str]]:
    """Processes (other than this one) with the database or its WAL open: (pid, command line)."""
    names = {str(db), str(db) + "-wal", str(db) + "-shm"}
    out = []
    for p in Path("/proc").iterdir():
        if not p.name.isdigit() or int(p.name) == os.getpid():
            continue
        try:
            fds = list((p / "fd").iterdir())
        except OSError:
            continue
        for fd in fds:
            try:
                if os.readlink(fd) in names:
                    cmd = (p / "cmdline").read_bytes().replace(b"\0", b" ").decode(errors="replace").strip()
                    out.append((int(p.name), cmd))
                    break
            except OSError:
                continue
    return out


def state(conn: sqlite3.Connection, key: str) -> str | None:
    r = conn.execute("SELECT value FROM state WHERE key=?", (key,)).fetchone()
    return r[0] if r else None


def protected_hash(conn: sqlite3.Connection, cutoff: str) -> dict[str, str]:
    """A digest per table of everything the rewrite must not touch."""
    out = {}
    tables = [r[0] for r in conn.execute("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")]
    for t in sorted(tables):
        if t == "candles":
            q = "SELECT * FROM candles WHERE source != 'replay' OR day >= ?"
            args = (cutoff,)
        elif t == "company_days":
            q, args = "SELECT * FROM company_days WHERE day >= ?", (cutoff,)
        elif t == "companies":
            cols = [r[1] for r in conn.execute("PRAGMA table_info(companies)") if r[1] != "ipo_price"]
            q, args = f"SELECT {', '.join(cols)} FROM companies", ()
        else:
            q, args = f'SELECT * FROM "{t}"', ()
        h = hashlib.sha256()
        for row in sorted(repr(tuple(r)) for r in conn.execute(q, args)):
            h.update(row.encode())
            h.update(b"\n")
        out[t] = h.hexdigest()
    return out


def backup(db: Path) -> Path:
    dest = db.with_name(f"{db.stem}-before-rebuild-{datetime.now().strftime('%Y%m%d-%H%M%S')}.db")
    src = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    out = sqlite3.connect(dest)
    with out:
        src.backup(out)
    out.close()
    src.close()
    return dest


def warm_inputs(work: Path, src: Path, collector: Path, window_start: str, until: str, warmup: int) -> Path:
    """The replay's inputs directory: the game's own files for the window (linked, every day required), plus
    `warmup` days before the window written here by engine/daily_inputs.py (same code, same point-in-time rules,
    window start moved back so the shows already on air are in them). The replay bootstraps the valuation over
    every file up to its start, so a show that premiered before the window starts from the filter state its
    observations up to then give, as the server's own valuation rebuild does (it reads 120 days), instead of
    from its pre-air prior (which made the first weeks of the history a slide)."""
    out = work / "inputs"
    shutil.rmtree(out, ignore_errors=True)
    out.mkdir(parents=True)
    d, end = date.fromisoformat(window_start), date.fromisoformat(until)
    missing = []
    while d <= end:
        f = src / f"{d.isoformat()}.json"
        if f.exists():
            (out / f.name).symlink_to(f.resolve())
        else:
            missing.append(d.isoformat())
        d += timedelta(days=1)
    if missing:
        sys.exit(f"daily inputs missing in {src}: {missing[0]}..{missing[-1]} ({len(missing)} days); regenerate them first")
    if warmup > 0:
        a = date.fromisoformat(window_start) - timedelta(days=warmup)
        b = date.fromisoformat(window_start) - timedelta(days=1)
        print(f"writing {warmup} days of warm-up inputs ({a}..{b}) ...", flush=True)
        env = dict(os.environ, ANISTOCK_INPUTS_DIR=str(out), ANISTOCK_DB=str(collector))
        subprocess.run([sys.executable, "engine/daily_inputs.py", "--from", a.isoformat(), "--day", b.isoformat(),
                        "--window-start", a.isoformat(), "--force"], cwd=ROOT, env=env, check=True, stdout=subprocess.DEVNULL)
    return out


def default_bin() -> Path:
    # production: deploy/binary.sh installs the release binary to bin/anistock; development: the cargo build
    for p in (ROOT / "bin" / "anistock", ROOT / "backend" / "target" / "release" / "anistock"):
        if p.exists():
            return p
    return ROOT / "bin" / "anistock"


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("db", type=Path, help="the game database (the server stopped)")
    ap.add_argument("--dry-run", action="store_true", help="compute and report; change nothing")
    ap.add_argument("--force", action="store_true", help="run even though another process has the database open")
    ap.add_argument("--inputs", type=Path, default=Path(os.environ.get("ANISTOCK_INPUTS_DIR", ROOT / "data" / "derived" / "inputs")),
                    help="daily inputs (default $ANISTOCK_INPUTS_DIR, else data/derived/inputs)")
    ap.add_argument("--collector", type=Path, default=Path(os.environ.get("ANISTOCK_DB", ROOT / "data" / "anistock.db")),
                    help="collector database (default $ANISTOCK_DB, else data/anistock.db)")
    ap.add_argument("--bin", type=Path, default=None, help="the server binary (default bin/anistock, else the cargo release build)")
    ap.add_argument("--work", type=Path, default=None, help="scratch directory (default: a new temporary directory, removed afterwards)")
    ap.add_argument("--warmup-days", type=int, default=120,
                    help="days of valuation history before the window (inputs generated here) for the shows already on air")
    ap.add_argument("--history", type=Path, default=None, help="reuse a replay-history JSON from an earlier run (skips steps 1-2)")
    args = ap.parse_args()

    db = args.db.resolve()
    if not db.exists():
        sys.exit(f"{db} does not exist")
    open_by = holders(db)
    if open_by:
        lines = "\n".join(f"  pid {pid}: {cmd[:120]}" for pid, cmd in open_by)
        if not args.force:
            sys.exit(f"{db} is open in another process (the server?):\n{lines}\n"
                     "stop the server first (systemctl --user stop anistock-server), or --force")
        print(f"warning: {db} is open in another process (--force):\n{lines}")

    # ---------------------------------------------------------------- the target: launch day, anchors
    conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    launched = state(conn, "launched_on")
    window_start = state(conn, "window_start")
    a_ref = state(conn, "a_ref")
    if not (launched and window_start and a_ref):
        sys.exit("the database has no launched_on / window_start / a_ref state: not a launched game")
    until = (date.fromisoformat(launched) - timedelta(days=1)).isoformat()
    bad = conn.execute("SELECT COUNT(*) FROM candles WHERE source='replay' AND day >= ?", (launched,)).fetchone()[0]
    bad_live = conn.execute("SELECT COUNT(*) FROM candles WHERE source!='replay' AND day < ?", (launched,)).fetchone()[0]
    if bad or bad_live:
        sys.exit(f"unexpected history: {bad} replay candles on/after launch {launched}, {bad_live} live candles before it")
    old = {}
    for cid, first, last, n in conn.execute(
            "SELECT company_id, MIN(day), MAX(day), COUNT(*) FROM candles WHERE source='replay' GROUP BY company_id"):
        close = conn.execute("SELECT close FROM candles WHERE company_id=? AND day=?", (cid, last)).fetchone()[0]
        first_close = conn.execute("SELECT close FROM candles WHERE company_id=? AND day=?", (cid, first)).fetchone()[0]
        live = conn.execute("SELECT day, open FROM candles WHERE company_id=? AND source!='replay' ORDER BY day LIMIT 1",
                            (cid,)).fetchone()
        title, ipo, status = conn.execute("SELECT title, ipo_price, status FROM companies WHERE id=?", (cid,)).fetchone()
        old[cid] = {"first": first, "last": last, "n": n, "anchor": close, "first_close": first_close, "title": title,
                    "ipo": ipo, "status": status, "live_day": live[0] if live else None, "live_open": live[1] if live else None}
    conn.close()
    if not old:
        sys.exit("no replay candles: nothing to rebuild")
    print(f"{db}: window {window_start}, launched {launched}; {len(old)} stocks with replay history "
          f"(rebuilding {window_start}..{until})")

    # ---------------------------------------------------------------- the new replay
    work = None if args.history else args.work or Path(tempfile.mkdtemp(prefix="rebuild-replay-"))
    try:
        hist_path = args.history or work / "history.json"
        if not args.history:
            work.mkdir(parents=True, exist_ok=True)
            start_db = work / "start.db"
            for suffix in ("", "-wal", "-shm"):
                Path(str(start_db) + suffix).unlink(missing_ok=True)
            env = dict(os.environ, ANISTOCK_GAME_DB=str(start_db), ANISTOCK_DB=str(args.collector))
            print(f"seeding the start world ({window_start}) ...", flush=True)
            subprocess.run([sys.executable, "-m", "server.seed", "--until", window_start], cwd=ROOT, env=env, check=True)
            w = sqlite3.connect(start_db)
            if state(w, "window_start") != window_start:
                sys.exit(f"the start world's window ({state(w, 'window_start')}) differs from the game's ({window_start})")
            w.execute("UPDATE state SET value=? WHERE key='a_ref'", (a_ref,))  # the game's demand reference
            w.commit()
            w.close()
            exe = args.bin or default_bin()
            inputs = warm_inputs(work, args.inputs, args.collector, window_start, until, args.warmup_days)
            cmd = [str(exe), "replay-history", "--world", str(start_db), "--inputs", str(inputs),
                   "--until", until, "--out", str(hist_path)]
            if args.collector.exists():
                cmd += ["--collector", str(args.collector)]
            print("replaying with the current model (about 15 minutes) ...", flush=True)
            subprocess.run(cmd, cwd=ROOT, env=dict(os.environ, ANISTOCK_ROOT=str(ROOT)), check=True)
        h = json.loads(hist_path.read_text(encoding="utf-8"))
    finally:
        if work is not None and args.work is None:
            shutil.rmtree(work, ignore_errors=True)
    if h["until"] != until:
        sys.exit(f"the replay ends on {h['until']}, the game launched on {launched}")

    candles: dict[int, list] = {}
    for cid, d, o, hi, lo, c, v in h["candles"]:
        if d <= until:
            candles.setdefault(cid, []).append((d, o, hi, lo, c, v))
    days: dict[int, list] = {}
    for cid, d, *rest in h["days"]:
        if d <= until:
            days.setdefault(cid, []).append((d, *rest))

    # ---------------------------------------------------------------- per stock: scale onto the anchor
    plan, skipped = {}, []
    for cid, o in sorted(old.items()):
        rows = sorted(candles.get(cid, []))
        info = h["companies"].get(str(cid))
        if not rows or info is None:
            skipped.append((cid, o["title"], "not listed in the new replay"))
            continue
        if rows[-1][0] != until:
            why = f"the new replay ends on {rows[-1][0]} ({info['status']})"
            skipped.append((cid, o["title"], why))
            continue
        k = o["anchor"] / max(1, rows[-1][4])
        sc = lambda x: max(1, round(x * k))  # noqa: E731
        new = [(d, sc(op), sc(hi), sc(lo), sc(c), v) for d, op, hi, lo, c, v in rows]
        assert new[-1][4] == o["anchor"]
        plan[cid] = {"k": k, "candles": new, "days": sorted(days.get(cid, [])), "ipo": sc(info["ipo_price"])}

    print()
    print(f"{'id':>7}  {'first close':>21}  {'last close':>21}  {'scale':>7}  {'IPO':>17}  {'days':>9}  live open  title")
    print(f"{'':>7}  {'old → new':>21}  {'old → new':>21}  {'':>7}  {'old → new':>17}  {'old→new':>9}")
    for cid, p in plan.items():
        o = old[cid]
        n = p["candles"]
        lo = "" if o["live_open"] is None else f"{o['live_open'] / 100:.2f}"
        print(f"{cid:>7}  {o['first_close'] / 100:>9.2f} → {n[0][4] / 100:>9.2f}  {o['anchor'] / 100:>9.2f} → {n[-1][4] / 100:>9.2f}"
              f"  {p['k']:>7.3f}  {o['ipo'] / 100:>7.2f} → {p['ipo'] / 100:>7.2f}  {o['n']:>4}→{len(n):<4}  {lo:>9}  {o['title'][:24]}")
    ks = sorted(p["k"] for p in plan.values())
    if ks:
        print(f"\n{len(plan)} stocks rebuilt; scale factor median {ks[len(ks) // 2]:.3f}, range {ks[0]:.3f}..{ks[-1]:.3f}")
    for cid, title, why in skipped:
        print(f"kept as is: {cid} {title[:30]}: {why}")
    if args.dry_run:
        print("\n--dry-run: nothing written")
        return

    # ---------------------------------------------------------------- write
    if not plan:
        sys.exit("nothing to write")
    bk = backup(db)
    print(f"\nbackup: {bk}")
    conn = sqlite3.connect(db, isolation_level=None)
    conn.execute("PRAGMA busy_timeout = 10000")
    conn.execute("BEGIN IMMEDIATE")
    try:
        before = protected_hash(conn, launched)
        ledger_before = conn.execute("SELECT COALESCE(SUM(cash), 0) FROM accounts").fetchone()[0]
        for cid, p in plan.items():
            conn.execute("DELETE FROM candles WHERE company_id=? AND source='replay' AND day < ?", (cid, launched))
            conn.executemany("INSERT INTO candles(company_id, day, open, high, low, close, volume, source) "
                             "VALUES (?,?,?,?,?,?,?,'replay')", [(cid, *r) for r in p["candles"]])
            conn.execute("DELETE FROM company_days WHERE company_id=? AND day < ?", (cid, launched))
            conn.executemany("INSERT INTO company_days(company_id, day, revenue, rev_us, rev_cn, rev_jp, aired, dividend_ps) "
                             "VALUES (?,?,?,?,?,?,?,?)", [(cid, *r) for r in p["days"]])
            conn.execute("UPDATE companies SET ipo_price=? WHERE id=?", (p["ipo"], cid))
        after = protected_hash(conn, launched)
        changed = [t for t in before if before[t] != after.get(t)]
        if changed or set(after) != set(before):
            raise RuntimeError(f"rows outside the replay history changed: {changed}")
        total = conn.execute("SELECT COALESCE(SUM(cash), 0) FROM accounts").fetchone()[0]
        if total != 0 or total != ledger_before:
            raise RuntimeError(f"cash does not sum to zero: {total}")
        conn.execute("COMMIT")
    except BaseException:
        conn.execute("ROLLBACK")
        raise
    finally:
        conn.close()
    print(f"rewrote the replay history of {len(plan)} stocks; untouched tables verified "
          f"({', '.join(sorted(t for t in before if t not in OWN))} and candles/company_days from {launched} on)")
    print("start the server again: systemctl --user start anistock-server")


if __name__ == "__main__":
    main()
