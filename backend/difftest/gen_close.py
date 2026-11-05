"""Generate the daily-close differential scenario (JSON lines) for a realistic game DB.

    python backend/difftest/gen_close.py BASE.db INPUTS_DIR SEED OUT.jsonl [LATE_DAY] [CATCHUP_FROM]

For each day with an inputs file: the opening auction phase (NPC ticks in auction mode, player limit
orders collected with match=False), the opening auction, continuous trading (NPC ticks with one shared
seeded random.Random, player limit/market orders priced relative to the live last price, cancels, IPO
subscribe/withdraw), then the close of the day at 06:30 the next morning. LATE_DAY is closed late: the
next day's auction and some trading happen first (the "late close must not overwrite" rule). From
CATCHUP_FROM on, the remaining days are closed together by one catch_up. A few "restart" ops make the
Rust side reload its whole state from the database.
"""

import json
import random
import sqlite3
import sys
from datetime import date, timedelta
from pathlib import Path


def main(base, inputs_dir, seed, out, late_day=None, catchup_from=None):
    rng = random.Random(seed)
    db = sqlite3.connect(f"file:{base}?mode=ro", uri=True)
    players = [r[0] for r in db.execute("SELECT id FROM accounts WHERE kind='player' ORDER BY id")]
    last_closed = db.execute("SELECT value FROM state WHERE key='last_closed'").fetchone()[0]
    days = sorted(p.stem for p in Path(inputs_dir).glob("*.json") if p.stem > last_closed)
    ops = [{"op": "rng", "seed": seed}]

    def ts(day, h, m, s=0):
        return f"{day}T{h:02d}:{m:02d}:{s:02d}+08:00"

    def player_ops(day, h, m, auction):
        for _ in range(rng.randint(0, 4)):
            acct = rng.choice(players)
            r = rng.random()
            t = ts(day, h, m, rng.randint(0, 59))
            if r < 0.55:
                kind = "limit" if auction or rng.random() < 0.7 else "market"
                ops.append({"op": "place_rel", "acct": acct, "co_idx": rng.randint(0, 10**6),
                            "side": rng.choice(["buy", "sell"]), "kind": kind,
                            "qty": rng.choice([rng.randint(1, 30), rng.randint(20, 400)]),
                            "rel": rng.choice([1.0, rng.uniform(0.9, 1.1), rng.uniform(0.7, 1.4)]),
                            "day": day, "ts": t, "match": not auction})
            elif r < 0.70:
                ops.append({"op": "cancel", "idx": rng.randint(0, 10**9), "recent": True, "own": True,
                            "acct": acct, "ts": t})
            elif r < 0.94:
                ops.append({"op": "subscribe", "acct": acct, "idx": rng.randint(0, 10**6),
                            "qty": rng.choice([0, rng.randint(1, 500), rng.randint(1000, 30000),
                                               rng.randint(20000, 60000)]),
                            "today": day, "ts": t})
            else:
                ops.append({"op": "unsubscribe", "acct": acct, "idx": rng.randint(0, 10**6)})

    def trading_day(day, morning_only=False):
        for i in range(4):                                   # 08:45-08:59 auction phase
            ops.append({"op": "npc_tick", "day": day, "ts": ts(day, 8, 45 + 4 * i), "auction": True})
            player_ops(day, 8, 45 + 4 * i, True)
        ops.append({"op": "auction", "day": day, "ts": ts(day, 9, 0)})
        n = 6 if morning_only else 36
        for i in range(n):                                   # every 20 minutes from 09:00
            h, m = 9 + (i * 20) // 60, (i * 20) % 60
            ops.append({"op": "npc_tick", "day": day, "ts": ts(day, h, m, 30), "auction": False})
            player_ops(day, h, m, False)
            if rng.random() < 0.01:
                ops.append({"op": "restart"})

    nxt = lambda d: (date.fromisoformat(d) + timedelta(days=1)).isoformat()   # noqa: E731
    pending_catchup = []
    for k, day in enumerate(days):
        if late_day and day == nxt(late_day):
            pass                                             # its morning was traded before the late close
        else:
            trading_day(day)
        if rng.random() < 0.5:
            ops.append({"op": "restart"})
        if catchup_from and day >= catchup_from:
            pending_catchup.append(day)
            if day == days[-1]:
                ops.append({"op": "catch_up", "today": nxt(day), "ts": ts(nxt(day), 6, 30), "inputs": inputs_dir})
            continue
        bad = Path(inputs_dir) / f"{day}.bad"
        if bad.exists():                                     # a close that fails half-way and rolls back
            ops.append({"op": "close", "day": day, "ts": ts(nxt(day), 6, 29), "inputs": str(bad)})
        if day == late_day:
            trading_day(nxt(day), morning_only=True)          # the next day is already trading
            ops.append({"op": "close", "day": day, "ts": ts(nxt(day), 11, 0),
                        "inputs": str(Path(inputs_dir) / f"{day}.json")})
            # then the rest of the next day's session
            for i in range(6, 36):
                h, m = 9 + (i * 20) // 60, (i * 20) % 60
                ops.append({"op": "npc_tick", "day": nxt(day), "ts": ts(nxt(day), h, m, 30), "auction": False})
                player_ops(nxt(day), h, m, False)
        else:
            ops.append({"op": "close", "day": day, "ts": ts(nxt(day), 6, 30),
                        "inputs": str(Path(inputs_dir) / f"{day}.json")})
        ops.append({"op": "close", "day": day, "ts": ts(nxt(day), 6, 31),
                    "inputs": str(Path(inputs_dir) / f"{day}.json")})   # idempotent: skipped
    with open(out, "w") as f:
        for op in ops:
            f.write(json.dumps(op, ensure_ascii=False) + "\n")
    print(f"{len(ops)} ops over {days[0]}..{days[-1]}")


if __name__ == "__main__":
    a = sys.argv[1:]
    main(a[0], a[1], int(a[2]), a[3], a[4] if len(a) > 4 and a[4] else None, a[5] if len(a) > 5 else None)
