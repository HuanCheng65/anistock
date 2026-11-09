"""Value a live-like world two ways and compare every listed stock: the check to run before any change to the
valuation, its parameters or its inputs reaches the game (`git show 99514a7:docs/restart-v0.6.6.md` and the 2026-11-07 repricings:
the replays of sim/quickcheck.py start from a 7/7 world and miss stocks only the seed lists, and a median hides
the few stocks a change moves several times over).

    python3 sim/replica_check.py --a inputs=DIR[,filter=FILE][,params=TOML] --b ... [--until DAY] [--top N]

Seeds a world the way the server's game was seeded (`server.seed --until DAY`, default: the last inputs file of
--a), then starts the server binary on a copy of it once per side with ANISTOCK_NO_LOOPS=1, so it bootstraps the
valuation from that side's inputs history (as at go-live or with ANISTOCK_VALUATION_REBUILD=1) and nothing trades;
reads every listed company's value. FILE is a valuation filter file (ANISTOCK_FILTER_FILE, relative to the repo);
TOML a file of dotted parameter overrides applied after server/game.toml's (ANISTOCK_PARAMS_FILE).
Prints the ratio B/A overall and by phase, the stocks it moves most, and every stock moved by more than 1.5x
either way, with what is known about it (long-runner, finished, markets with data).
"""

from __future__ import annotations

import argparse
import json
import math
import os
import random
import shutil
import sqlite3
import statistics as st
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WORK = ROOT / "data" / "fast" / "replica_check"
BIN = (Path(os.environ["REPLICA_BIN"]) if os.environ.get("REPLICA_BIN") else
       next((p for p in (ROOT / "backend/target/release/anistock", ROOT / "bin/anistock") if p.exists()), None))


def side(spec: str) -> dict:
    out = {}
    for part in spec.split(","):
        k, _, v = part.partition("=")
        out[k.strip()] = v.strip()
    if "inputs" not in out:
        sys.exit(f"{spec}: inputs=DIR is required")
    return out


def value(seed_db: Path, name: str, s: dict) -> dict[int, int]:
    db = WORK / f"{name}.db"
    for suffix in ("", "-wal", "-shm"):
        Path(str(db) + suffix).unlink(missing_ok=True)
    shutil.copy(seed_db, db)
    env = dict(os.environ, ANISTOCK_GAME_DB=str(db), ANISTOCK_ROOT=os.environ.get("REPLICA_SERVER_ROOT", str(ROOT)), ANISTOCK_NO_LOOPS="1",
               ANISTOCK_INPUTS_DIR=str(Path(s["inputs"]).resolve()), ANISTOCK_PORT=str(random.randint(20000, 29999)),
               ANISTOCK_DB=os.environ.get("ANISTOCK_DB", str(ROOT / "data" / "anistock.db")))
    if s.get("filter"):
        env["ANISTOCK_FILTER_FILE"] = s["filter"]
    if s.get("params"):
        env["ANISTOCK_PARAMS_FILE"] = str(Path(s["params"]).resolve())
    p = subprocess.Popen([str(BIN)], cwd=ROOT, env=env, stderr=subprocess.PIPE, stdout=subprocess.DEVNULL, text=True)
    ok, log = False, []
    t0 = time.time()
    for line in p.stderr:
        log.append(line)
        if "valuation: started the filter" in line:
            ok = True
        if "listening on" in line or time.time() - t0 > 120:
            break
    p.terminate()
    p.wait(timeout=30)
    if not ok:
        sys.exit(f"{name}: the server did not bootstrap the valuation:\n{''.join(log[-15:])}")
    c = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    return {i: f for i, f in c.execute("SELECT id, fair_value FROM companies WHERE status='listed' AND fair_value > 0")}


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--a", required=True, type=side)
    ap.add_argument("--b", required=True, type=side)
    ap.add_argument("--until", default=None, help="last day of the seeded world (default: --a's last inputs file)")
    ap.add_argument("--top", type=int, default=20)
    args = ap.parse_args()
    if BIN is None:
        sys.exit("no server binary: cargo build --release (in backend/)")
    until = args.until or max(f.stem for f in Path(args.a["inputs"]).glob("*.json"))
    WORK.mkdir(parents=True, exist_ok=True)
    seed_db = WORK / f"seed-{until}.db"
    if not seed_db.exists():
        env = dict(os.environ, ANISTOCK_GAME_DB=str(seed_db))
        r = subprocess.run(["uv", "run", "python", "-m", "server.seed", "--force", "--until", until], cwd=ROOT, env=env,
                           capture_output=True, text=True)
        if r.returncode != 0:
            sys.exit(f"seed failed:\n{r.stderr[-2000:]}")
        print(r.stdout.strip().splitlines()[-1])
    t = time.time()
    a, b = value(seed_db, "a", args.a), value(seed_db, "b", args.b)
    print(f"valued twice in {time.time() - t:.0f}s")

    c = sqlite3.connect(f"file:{seed_db}?mode=ro", uri=True)
    title = {i: t for i, t in c.execute("SELECT id, title FROM companies")}
    shows = {x["id"]: x for x in json.loads((Path(args.b["inputs"]) / f"{until}.json").read_text())["shows"]}

    def tag(i: int) -> str:
        s = shows.get(i, {})
        if s.get("long_runner"):
            k = "长篇"
        elif s.get("episodes") and s.get("aired", 0) >= s["episodes"]:
            k = "完结"
        elif s.get("aired", 0) == 0:
            k = "未播"
        else:
            k = "播出"
        src = "".join(m for m in ("cn", "jp") if s.get("sources", {}).get(m)) or "us"
        return f"{k} {src}"

    ids = sorted(set(a) & set(b))
    only = sorted(set(a) ^ set(b))
    rows = [(i, b[i] / a[i]) for i in ids]
    r = sorted(x for _, x in rows)
    n = len(r)
    print(f"\n{n} 只  B/A 中位 {st.median(r):.3f}  P10 {r[n // 10]:.3f}  P90 {r[n * 9 // 10]:.3f}  最小 {r[0]:.3f}  最大 {r[-1]:.3f}"
          + (f"；只在一边有估值：{only}" if only else ""))
    by: dict[str, list[float]] = {}
    for i, x in rows:
        by.setdefault(tag(i).split()[0], []).append(x)
    print("按类别中位：" + "  ".join(f"{k} {st.median(v):.2f}（{len(v)}）" for k, v in sorted(by.items())))
    print(f"\n变化最大的 {args.top} 支：")
    print(f"{'id':>7} {'作品':<18} {'类别':<9} {'A':>9} {'B':>9} {'B/A':>6}")
    for i, x in sorted(rows, key=lambda t: -abs(math.log(t[1])))[:args.top]:
        print(f"{i:>7} {title.get(i, '')[:18]:<18} {tag(i):<9} {a[i] / 100:>9.2f} {b[i] / 100:>9.2f} {x:>6.2f}")
    big = [i for i, x in rows if x > 1.5 or x < 1 / 1.5]
    print(f"\n变化超过 1.5 倍的 {len(big)} 支" + ("：逐一核对原因后再上线" if big else "。"))


if __name__ == "__main__":
    main()
