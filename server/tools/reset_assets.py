"""Start over with the players' accounts kept and their assets reset: copy every player of an old game into a
freshly seeded one.

    python -m server.tools.reset_assets OLD_DB NEW_DB [--dry-run]

NEW_DB is a new game (`server.seed --force`) already migrated to the current schema (run `anistock announce`
on it first: it migrates the database and posts the announcement the players will see). Run with the server
stopped. For each player of OLD_DB (closed accounts are not copied), NEW_DB gets:

- the account: username, password hash and opening time (so 身份验证's day counts and the announcement, which is
  newer than every carried account, still apply), with the starting cash granted from the world account like a
  registration; holdings, orders, credit, restructures and every other game record start empty;
- what belongs to the person, not the game: the QQ binding (身份验证), the signed documents, the risk assessments
  and knowledge tests, the login IP history (the per-IP limits);
- a password-change time of now, so no session cookie of the old game is accepted (the account ids change).

Player ids are renumbered above every id the old game used, so an old cookie could never name a new player
either. The cash invariants are checked before the commit.
"""

from __future__ import annotations

import argparse
import sqlite3
import sys
import time
from pathlib import Path

from .. import db as gdb
from .. import exchange as ex
from ..config import Clock, load

# person-level tables carried over, by account_id; an `id` primary key is renumbered by SQLite
CARRY = ["bindings", "account_agreements", "risk_assessments", "quiz_results", "account_logins"]


def columns(conn: sqlite3.Connection, table: str) -> list[str]:
    return [r[1] for r in conn.execute(f"PRAGMA table_info({table})")]


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("old", type=Path)
    ap.add_argument("new", type=Path)
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()
    cfg = load()

    old = sqlite3.connect(f"file:{args.old.resolve()}?mode=ro", uri=True)
    old.row_factory = sqlite3.Row
    new = gdb.connect(args.new)
    have = {r[0] for r in new.execute("SELECT name FROM sqlite_master WHERE type='table'")}
    missing = [t for t in CARRY + ["account_security", "announcements"] if t not in have]
    if missing:
        sys.exit(f"{args.new} lacks {missing}: run `anistock announce` on it first (it migrates the database)")
    if new.execute("SELECT COUNT(*) FROM accounts WHERE kind='player'").fetchone()[0]:
        sys.exit(f"{args.new} already has players: it should be a fresh seed")
    launched = gdb.get_state(new, "launched_on")
    if not launched:
        sys.exit(f"{args.new} has no launched_on: not a seeded game")
    old_tables = {r[0] for r in old.execute("SELECT name FROM sqlite_master WHERE type='table'")}

    players = old.execute("SELECT id, name, username, pw_hash, created_at FROM accounts WHERE kind='player' ORDER BY id").fetchall()
    top = max(old.execute("SELECT COALESCE(MAX(id), 0) FROM accounts").fetchone()[0],
              new.execute("SELECT COALESCE(MAX(id), 0) FROM accounts").fetchone()[0])
    remap = {p["id"]: top + 1 + i for i, p in enumerate(players)}
    grant = round(cfg.game["initial_cash"] * 100)
    ts = Clock(cfg.tz, cfg.game.get("day_rollover", "00:00")).ts()
    now = int(time.time())

    new.execute("BEGIN IMMEDIATE")
    try:
        world = gdb.special(new, "world")
        for p in players:
            nid = remap[p["id"]]
            new.execute("INSERT INTO accounts(id, kind, name, username, pw_hash, cash, cash_frozen, created_at) "
                        "VALUES (?, 'player', ?, ?, ?, 0, 0, ?)", (nid, p["name"], p["username"], p["pw_hash"], p["created_at"]))
            ex.transfer(new, world, nid, grant, "grant", day=launched, ts=ts)
            new.execute("INSERT INTO account_security(account_id, pw_changed_at) VALUES (?, ?)", (nid, now))
        carried = {}
        for t in CARRY:
            if t not in old_tables:
                continue
            cols = [c for c in columns(old, t) if c in columns(new, t)]
            # an INTEGER PRIMARY KEY `id` gets a fresh value (the tables keep nothing that points at it)
            cols = [c for c in cols if c != "id"]
            n = 0
            for r in old.execute(f"SELECT {', '.join(cols)} FROM {t}"):
                row = dict(zip(cols, r))
                if row["account_id"] not in remap:
                    continue        # a closed account
                row["account_id"] = remap[row["account_id"]]
                new.execute(f"INSERT INTO {t}({', '.join(cols)}) VALUES ({', '.join('?' * len(cols))})", [row[c] for c in cols])
                n += 1
            carried[t] = n
        gdb.check_invariants(new)
        if args.dry_run:
            new.execute("ROLLBACK")
        else:
            new.execute("COMMIT")
    except BaseException:
        if new.in_transaction:
            new.execute("ROLLBACK")
        raise
    print(f"{len(players)} players carried into {args.new} (ids {top + 1}..{top + len(players)}), "
          f"{grant / 100:,.0f} each; " + ", ".join(f"{t} {n}" for t, n in carried.items()))
    if args.dry_run:
        print("--dry-run: rolled back")


if __name__ == "__main__":
    main()
