"""Game database: schema and connection helpers.

Money and prices are integers in cents (0.01 币). Every movement of cash between accounts is a
ledger row, and two special accounts make the books close exactly:

    world   the outside economy: external revenue comes from here, operating costs, studio fees and
            rights holders' dividends go back here
    cb      the central bank: interest is created here

So the sum of `accounts.cash` over every account is always 0, and the shares of each company summed
over all holders always equal `companies.shares`. `check_invariants` asserts both.
"""

from __future__ import annotations

import sqlite3
from datetime import datetime
from pathlib import Path

MIGRATIONS = Path(__file__).resolve().parent / "migrations"


def connect(path: Path | str) -> sqlite3.Connection:
    conn = sqlite3.connect(str(path), isolation_level=None, check_same_thread=False)
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("PRAGMA foreign_keys=OFF")
    conn.execute("PRAGMA busy_timeout=5000")
    return conn


def init(db_path: Path | str) -> list[str]:
    """Apply pending schema migrations (server/migrations, via yoyo). If the database already has players,
    it is backed up first. Returns the ids of the migrations applied."""
    from yoyo import get_backend, read_migrations
    db_path = Path(db_path)
    db_path.parent.mkdir(parents=True, exist_ok=True)
    backend = get_backend(f"sqlite:///{db_path.resolve()}")
    migrations = read_migrations(str(MIGRATIONS))
    with backend.lock():
        pending = backend.to_apply(migrations)
        if pending and db_path.exists():
            conn = connect(db_path)
            has_players = conn.execute(
                "SELECT 1 FROM sqlite_master WHERE name='accounts'").fetchone() is not None
            if has_players:
                stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
                backup(conn, db_path.parent / "backups" / f"{db_path.stem}-before-migration-{stamp}.db")
            conn.close()
        backend.apply_migrations(pending)
    return [m.id for m in pending]


def backup(conn: sqlite3.Connection, dest: Path) -> Path:
    """Consistent online copy (SQLite backup API); safe while the game is running."""
    dest.parent.mkdir(parents=True, exist_ok=True)
    out = sqlite3.connect(str(dest))
    with out:
        conn.backup(out)
    out.close()
    return dest


def get_state(conn, key: str, default: str | None = None) -> str | None:
    row = conn.execute("SELECT value FROM state WHERE key=?", (key,)).fetchone()
    return row["value"] if row else default


def set_state(conn, key: str, value) -> None:
    conn.execute("INSERT INTO state(key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                 (key, str(value)))


def special(conn, kind: str) -> int:
    """Id of a singleton account (world, cb, gov)."""
    row = conn.execute("SELECT id FROM accounts WHERE kind=?", (kind,)).fetchone()
    if row is None:
        raise KeyError(f"no {kind} account")
    return row["id"]


def check_invariants(conn) -> None:
    total = conn.execute("SELECT COALESCE(SUM(cash), 0) FROM accounts").fetchone()[0]
    assert total == 0, f"cash does not sum to zero: {total}"
    bad = conn.execute("""
        SELECT c.id, c.shares, COALESCE(SUM(h.qty), 0) AS held FROM companies c
        LEFT JOIN holdings h ON h.company_id = c.id
        WHERE c.status = 'listed' GROUP BY c.id HAVING held != c.shares""").fetchall()
    assert not bad, f"share count mismatch: {[tuple(r) for r in bad][:5]}"
    lot_bad = conn.execute("""
        SELECT h.account_id, h.company_id, h.qty, COALESCE(SUM(l.qty), 0) AS lq FROM holdings h
        LEFT JOIN lots l ON l.account_id = h.account_id AND l.company_id = h.company_id
        GROUP BY h.account_id, h.company_id HAVING lq != h.qty""").fetchall()
    assert not lot_bad, f"lots do not match holdings: {[tuple(r) for r in lot_bad][:5]}"
    neg = conn.execute("""SELECT id, kind, cash, cash_frozen FROM accounts
                          WHERE kind NOT IN ('world', 'cb') AND (cash < 0 OR cash_frozen < 0 OR cash_frozen > cash)""").fetchall()
    assert not neg, f"negative or over-frozen balances: {[tuple(r) for r in neg][:5]}"
