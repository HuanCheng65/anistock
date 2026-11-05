"""Generate a seeded random operation sequence for the differential test (JSON lines).

    python backend/difftest/gen.py META.json SEED N_OPS OUT.jsonl

Each trading day: an auction phase from 08:45 (limit orders collected with match=False, market orders
rejected), the opening auction at 09:00 (v1 `ensure_opening_auction`), then continuous trading with the
clock advancing irregularly, sometimes landing exactly on VCM window / cooling-off boundaries. Prices
cluster around a drifting per-stock level and regularly jump 8-40% away from it to trip the volatility
control. Also: cancels (own, others', closed, unknown), cancel-all, transfers, share moves (FIFO lots),
and malformed orders that must be rejected with v1's messages.
"""

import json
import random
import sys
from datetime import date, datetime, timedelta, timezone

TZ = timezone(timedelta(hours=8))


def main(meta_path: str, seed: int, n: int, out: str) -> None:
    meta = json.load(open(meta_path))
    rng = random.Random(seed)
    players = meta["players"]
    level = {int(k): float(v) for k, v in meta["companies"].items()}
    cids = list(level)
    day = date.fromisoformat(meta["day0"])
    now = datetime(day.year, day.month, day.day, 8, 45, tzinfo=TZ)
    phase = "auction"
    ops = []
    day_ops = rng.randint(150, 600)
    placed = 0

    def ts():
        return now.isoformat(timespec="seconds")

    def price_for(cid):
        lv = level[cid]
        r = rng.random()
        if r < 0.55:
            p = lv * (1 + rng.gauss(0, 0.03))
        elif r < 0.85:
            p = lv * (1 + rng.choice([-1, 1]) * rng.uniform(0.08, 0.13))
        elif r < 0.97:
            p = lv * (1 + rng.choice([-1, 1]) * rng.uniform(0.15, 0.45))
        else:
            return rng.choice([0, -5, None, int(max(lv, 5) * 20) + rng.randint(0, 3), 100, 101, 1])
        return max(1, int(round(p)))

    while len(ops) < n:
        if phase == "auction" and now.hour == 9:
            ops.append({"op": "auction", "day": day.isoformat(), "ts": ts()})
            phase = "continuous"
            continue
        if phase == "continuous" and day_ops <= 0:
            day += timedelta(days=1)
            now = datetime(day.year, day.month, day.day, 8, 45, tzinfo=TZ) + timedelta(seconds=rng.randint(0, 600))
            phase = "auction"
            day_ops = rng.randint(150, 600)
            continue
        day_ops -= 1

        # advance the clock
        r = rng.random()
        if phase == "auction":
            now = min(now + timedelta(seconds=rng.randint(0, 20)),
                      datetime(day.year, day.month, day.day, 9, 0, tzinfo=TZ))
        elif r < 0.30:
            pass
        elif r < 0.85:
            now += timedelta(seconds=rng.randint(1, 40))
        elif r < 0.93:
            now += timedelta(minutes=rng.choice([1, 2, 3, 4, 5, 6]))   # lands on VCM boundaries
        else:
            now += timedelta(seconds=rng.randint(60, 900))
        for cid in cids:
            level[cid] = max(1.0, level[cid] * (1 + rng.gauss(0, 0.004)))

        cid = rng.choice(cids)
        acct = rng.choice(players)
        if rng.random() < 0.003:
            ops.append({"op": "restart"})   # Rust reloads its whole state from the database here
            continue
        r = rng.random()
        if r < 0.70:
            side = rng.choice(["buy", "sell"])
            kind = "market" if (phase == "continuous" and rng.random() < 0.18) or rng.random() < 0.02 else "limit"
            qty = rng.choice([rng.randint(1, 20), rng.randint(1, 120), rng.randint(100, 2000)]) \
                if rng.random() > 0.02 else rng.choice([0, -3])
            price = price_for(cid) if kind == "limit" else (None if rng.random() < 0.9 else 500)
            op = {"op": "place", "acct": acct, "co": cid, "side": side, "kind": kind, "qty": qty, "price": price,
                  "day": day.isoformat(), "ts": ts(), "match": phase == "continuous"}
            if kind == "limit" and rng.random() < 0.15:
                # a price exactly on (or one cent around) a VCM edge, resolved by the drivers from the
                # company's current state
                op["edge"] = rng.choice(["anchor_hi", "anchor_lo", "band_hi", "band_lo", "last_hi", "last_lo"])
                op["off"] = rng.choice([-1, 0, 0, 1])
            x = rng.random()
            if x < 0.01:
                op["side"] = "hold"
            elif x < 0.02:
                op["kind"] = "stop"
            elif x < 0.035:
                op["co"] = rng.choice(meta["unlisted"])
            elif x < 0.04:
                op["acct"] = 9999
            ops.append(op)
            placed += 1
        elif r < 0.86:
            ops.append({"op": "cancel", "idx": rng.randint(0, 10**9), "recent": rng.random() < 0.7,
                        "own": rng.random() < 0.85, "acct": acct, "ts": ts()})
        elif r < 0.88:
            ops.append({"op": "cancel_raw", "acct": acct, "order_id": rng.choice([0, -1, 10**6, rng.randint(1, placed + 1)]),
                        "ts": ts()})
        elif r < 0.92:
            ops.append({"op": "cancel_all", "acct": acct, "co": cid, "ts": ts()})
        elif r < 0.96:
            ops.append({"op": "transfer", "from": acct, "to": rng.choice(players),
                        "frac": rng.random(), "raw": rng.choice([None] * 8 + [0, -1]),
                        "reason": "transfer", "day": day.isoformat(), "ts": ts()})
        elif r < 0.995:
            ops.append({"op": "move_shares", "from": acct, "to": rng.choice(players), "co": cid,
                        "frac": rng.random(), "day": day.isoformat()})
        else:
            ops.append({"op": "remove_too_many", "acct": acct, "co": cid})
    with open(out, "w") as f:
        for op in ops[:n]:
            f.write(json.dumps(op, ensure_ascii=False) + "\n")


if __name__ == "__main__":
    main(sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), sys.argv[4])
