"""Credit players a one-off sum from the world account (server stopped), e.g. the losses a rule change caused.

    uv run python -m server.tools.compensate data/game.db USER=YUAN [USER=YUAN ...] [--dry-run]

Each credit is a ledger row from `world` to the player (reason `grant`, the same as the registration grant) and the
player's cash rises by it; it counts in the player's total assets, so the ranking's return recovers with it. Run with
the server stopped (the engine reads the counters and balances when it opens). A backup is written to
data/backups/ first. 2026-11-09: the China market's switch to viewers repriced every stock; the players who lost were
paid the difference of their total assets before and after (docs/ops/switch-cn-views.md).
"""

from __future__ import annotations

import argparse
import sqlite3
import sys
from datetime import datetime
from pathlib import Path
from zoneinfo import ZoneInfo


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("db", type=Path)
    ap.add_argument("credits", nargs="+", help="USER=YUAN (username, or display name for accounts without one)")
    ap.add_argument("--dry-run", action="store_true")
    a = ap.parse_args()
    c = sqlite3.connect(a.db)
    world = c.execute("SELECT id FROM accounts WHERE kind='world'").fetchone()
    if not world:
        sys.exit("no world account")
    plan = []
    for item in a.credits:
        user, _, yuan = item.rpartition("=")
        cents = round(float(yuan) * 100)
        row = c.execute("SELECT id, cash FROM accounts WHERE kind='player' AND (username=? OR name=?)", (user, user)).fetchall()
        if len(row) != 1:
            sys.exit(f"{user}: {len(row)} player accounts match")
        if cents <= 0:
            sys.exit(f"{user}: amount must be positive")
        plan.append((user, row[0][0], row[0][1], cents))
    for user, _, cash, cents in plan:
        print(f"{user}: cash {cash / 100:,.2f} + {cents / 100:,.2f} = {(cash + cents) / 100:,.2f}")
    if a.dry_run:
        return
    backups = a.db.parent / "backups"
    backups.mkdir(exist_ok=True)
    dest = backups / f"{a.db.stem}-before-compensate-{datetime.now():%Y%m%d-%H%M%S}.db"
    with sqlite3.connect(dest) as b:
        c.backup(b)
    now = datetime.now(ZoneInfo("Asia/Shanghai"))
    ts = now.isoformat(timespec="seconds")
    day = c.execute("SELECT value FROM state WHERE key='last_closed'").fetchone()
    day = day[0] if day else now.date().isoformat()
    with c:
        for user, aid, _, cents in plan:
            nid = (c.execute("SELECT COALESCE(MAX(id), 0) FROM ledger").fetchone()[0]) + 1
            c.execute("INSERT INTO ledger(id, ts, day, from_id, to_id, amount, reason, company_id, ref) VALUES (?,?,?,?,?,?,?,?,?)",
                      (nid, ts, day, world[0], aid, cents, "grant", None, None))
            c.execute("UPDATE accounts SET cash = cash + ? WHERE id=?", (cents, aid))
            c.execute("UPDATE accounts SET cash = cash - ? WHERE id=?", (cents, world[0]))
    print(f"backup {dest}; credited {len(plan)} players")


if __name__ == "__main__":
    main()
