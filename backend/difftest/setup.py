"""Build a fresh game.db for the differential test with v1's own code (yoyo migration + exchange helpers).

    uv run python backend/difftest/setup.py OUT.db META.json [SEED]

Companies: four listed (one with a tiny previous close so the sanity band floors at 1.00), one delisted,
one still subscribing. Accounts: world/cb/gov, players with very different cash and shares (some nearly
broke, so market buys run out of money), and an inert holder per company so the share invariant holds.
"""

import json
import random
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))

from server import db as gdb  # noqa: E402
from server import exchange as ex  # noqa: E402

DAY, TS = "2026-11-04", "2026-11-04T20:00:00+08:00"


def main(out: str, meta_path: str, seed: int = 0) -> None:
    rng = random.Random(seed)
    path = Path(out)
    if path.exists():
        path.unlink()
    gdb.init(path)
    c = gdb.connect(path)
    c.execute("BEGIN")
    for kind in ("world", "cb", "gov"):
        c.execute("INSERT INTO accounts(kind, name, created_at) VALUES (?,?,?)", (kind, kind, TS))
    world = gdb.special(c, "world")
    companies = [  # id, title, status, shares, prev_close
        (101, "葬送のフリーレン", "listed", 10000, 500),
        (102, "药屋少女的呢喃", "listed", 10000, 1234),
        (103, "Tiny", "listed", 5000, 3),
        (104, "迷宫饭", "listed", 8000, 88),
        (105, "退市番", "delisted", 1000, 400),
        (106, "认购中", "subscribing", 1000, 400),
    ]
    for i, (cid, title, status, shares, pc) in enumerate(companies):
        c.execute("""INSERT INTO companies(id, code, title, first_air, listed_on, status, shares, ipo_price,
                     prev_close, last_price, open_ref) VALUES (?,?,?,?,?,?,?,?,?,?,?)""",
                  (cid, str(100001 + i), title, "2026-10-01", "2026-09-10", status, shares, pc, pc, pc,
                   pc if cid != 104 else None))
    players = []
    cash_levels = [5_000_000, 2_000_000, 1_000_000, 300_000, 100_000, 20_000, 3_000, 500, 0, 10_000_000]
    for i, cash in enumerate(cash_levels):
        pid = c.execute("INSERT INTO accounts(kind, name, username, created_at) VALUES ('player', ?, ?, ?)",
                        (f"p{i}", f"p{i}", TS)).lastrowid
        ex.transfer(c, world, pid, cash, "grant", day=DAY, ts=TS)
        players.append(pid)
    listed = [co for co in companies if co[2] == "listed"]
    for cid, _, _, shares, pc in listed:
        left = shares
        for pid in players:
            if rng.random() < 0.75:
                # several lots per holder so FIFO consumption crosses lot boundaries
                for _ in range(rng.randint(1, 3)):
                    q = min(left, rng.randint(1, shares // 25))
                    if q > 0:
                        ex.add_shares(c, pid, cid, q, max(1, pc + rng.randint(-pc // 5, pc // 5)),
                                      f"2026-10-{rng.randint(1, 28):02d}")
                        left -= q
        holder = c.execute("INSERT INTO accounts(kind, name, company_id, created_at) VALUES ('committee', ?, ?, ?)",
                           (f"holder{cid}", cid, TS)).lastrowid
        ex.add_shares(c, holder, cid, left, pc, "2026-09-10")
    # the delisted company's shares are gone; the subscribing one has none yet
    c.execute("COMMIT")
    gdb.check_invariants(c)
    meta = {"players": players, "companies": {str(co[0]): co[4] for co in listed},
            "unlisted": [105, 106, 999], "day0": "2026-11-05"}
    Path(meta_path).write_text(json.dumps(meta))


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2], int(sys.argv[3]) if len(sys.argv) > 3 else 0)
