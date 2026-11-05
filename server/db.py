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
from pathlib import Path

SCHEMA = """
CREATE TABLE IF NOT EXISTS accounts (
    id          INTEGER PRIMARY KEY,
    kind        TEXT NOT NULL,      -- player | world | cb | gov | company | committee | mm | noise
    name        TEXT NOT NULL,
    username    TEXT UNIQUE,        -- players only
    pw_hash     TEXT,
    company_id  INTEGER,            -- company / committee / mm accounts
    cash        INTEGER NOT NULL DEFAULT 0,
    cash_frozen INTEGER NOT NULL DEFAULT 0,   -- reserved by open buy orders
    created_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS accounts_kind ON accounts(kind);

CREATE TABLE IF NOT EXISTS companies (
    id            INTEGER PRIMARY KEY,   -- AniList media id
    code          TEXT UNIQUE NOT NULL,  -- display code, e.g. 100001
    title         TEXT NOT NULL,
    title_en      TEXT,
    studio        TEXT,
    episodes      INTEGER,
    long_runner   INTEGER NOT NULL DEFAULT 0,
    first_air     TEXT NOT NULL,
    listed_on     TEXT NOT NULL,
    status        TEXT NOT NULL,          -- listed | delisted
    shares        INTEGER NOT NULL,
    ipo_price     INTEGER NOT NULL,
    prev_close    INTEGER NOT NULL,       -- reference for the daily price limit
    last_price    INTEGER NOT NULL,
    fair_value    INTEGER NOT NULL DEFAULT 0,  -- engine valuation; hidden from players
    profit_week   INTEGER NOT NULL DEFAULT 0,  -- after-tax profit accrued since the last dividend
    low_weeks     INTEGER NOT NULL DEFAULT 0,  -- consecutive weeks below the delisting threshold
    delisted_on   TEXT
);

CREATE TABLE IF NOT EXISTS holdings (
    account_id  INTEGER NOT NULL,
    company_id  INTEGER NOT NULL,
    qty         INTEGER NOT NULL DEFAULT 0,
    qty_frozen  INTEGER NOT NULL DEFAULT 0,   -- reserved by open sell orders
    PRIMARY KEY (account_id, company_id)
);

-- FIFO lots: holding period decides the dividend tax rate
CREATE TABLE IF NOT EXISTS lots (
    id          INTEGER PRIMARY KEY,
    account_id  INTEGER NOT NULL,
    company_id  INTEGER NOT NULL,
    qty         INTEGER NOT NULL,
    cost        INTEGER NOT NULL,          -- price per share, cents
    acquired_on TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS lots_owner ON lots(account_id, company_id, id);

CREATE TABLE IF NOT EXISTS orders (
    id          INTEGER PRIMARY KEY,
    account_id  INTEGER NOT NULL,
    company_id  INTEGER NOT NULL,
    side        TEXT NOT NULL,             -- buy | sell
    kind        TEXT NOT NULL,             -- limit | market
    price       INTEGER,                   -- limit price; NULL for market orders
    qty         INTEGER NOT NULL,
    filled      INTEGER NOT NULL DEFAULT 0,
    frozen      INTEGER NOT NULL DEFAULT 0, -- cash (buy) still reserved
    status      TEXT NOT NULL,             -- open | filled | cancelled
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS orders_book ON orders(company_id, status, side, price, id);
CREATE INDEX IF NOT EXISTS orders_owner ON orders(account_id, status);

CREATE TABLE IF NOT EXISTS trades (
    id          INTEGER PRIMARY KEY,
    company_id  INTEGER NOT NULL,
    price       INTEGER NOT NULL,
    qty         INTEGER NOT NULL,
    buyer_id    INTEGER NOT NULL,
    seller_id   INTEGER NOT NULL,
    buy_order   INTEGER,
    sell_order  INTEGER,
    ts          TEXT NOT NULL,
    day         TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS trades_company ON trades(company_id, id);
CREATE INDEX IF NOT EXISTS trades_buyer ON trades(buyer_id, id);
CREATE INDEX IF NOT EXISTS trades_seller ON trades(seller_id, id);

CREATE TABLE IF NOT EXISTS ledger (
    id          INTEGER PRIMARY KEY,
    ts          TEXT NOT NULL,
    day         TEXT NOT NULL,
    from_id     INTEGER NOT NULL,
    to_id       INTEGER NOT NULL,
    amount      INTEGER NOT NULL,          -- cents, always > 0
    reason      TEXT NOT NULL,             -- trade | commission | stamp_duty | dividend | dividend_tax |
                                           -- revenue | operating_cost | corp_tax | interest | transfer |
                                           -- ipo | production_fee | delisting | grant
    company_id  INTEGER,
    ref         INTEGER                    -- trade id etc.
);
CREATE INDEX IF NOT EXISTS ledger_to ON ledger(to_id, id);
CREATE INDEX IF NOT EXISTS ledger_from ON ledger(from_id, id);

CREATE TABLE IF NOT EXISTS candles (
    company_id  INTEGER NOT NULL,
    day         TEXT NOT NULL,
    open        INTEGER NOT NULL,
    high        INTEGER NOT NULL,
    low         INTEGER NOT NULL,
    close       INTEGER NOT NULL,
    volume      INTEGER NOT NULL DEFAULT 0,
    source      TEXT NOT NULL DEFAULT 'live',   -- live | replay (pre-launch history from the simulation)
    PRIMARY KEY (company_id, day)
);

-- per company per day: what the business did (public information)
CREATE TABLE IF NOT EXISTS company_days (
    company_id  INTEGER NOT NULL,
    day         TEXT NOT NULL,
    revenue     INTEGER NOT NULL,
    rev_us      INTEGER NOT NULL,
    rev_cn      INTEGER NOT NULL,
    rev_jp      INTEGER NOT NULL,
    aired       INTEGER NOT NULL,
    dividend_ps INTEGER NOT NULL DEFAULT 0,      -- dividend per share paid that day (cents)
    PRIMARY KEY (company_id, day)
);

CREATE TABLE IF NOT EXISTS macro_days (
    day         TEXT PRIMARY KEY,
    rate        REAL NOT NULL,             -- policy rate, annual
    m2          INTEGER NOT NULL,
    m2_growth   REAL,
    injected    INTEGER NOT NULL,
    exported    INTEGER NOT NULL,
    taxes       INTEGER NOT NULL,
    interest    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS news (
    id          INTEGER PRIMARY KEY,
    ts          TEXT NOT NULL,
    day         TEXT NOT NULL,
    kind        TEXT NOT NULL,             -- ipo | premiere | finale | dividend | rate | delisting | system
    company_id  INTEGER,
    title       TEXT NOT NULL,
    body        TEXT
);
CREATE INDEX IF NOT EXISTS news_day ON news(id);

CREATE TABLE IF NOT EXISTS state (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"""


def connect(path: Path | str) -> sqlite3.Connection:
    conn = sqlite3.connect(str(path), isolation_level=None, check_same_thread=False)
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("PRAGMA foreign_keys=OFF")
    conn.execute("PRAGMA busy_timeout=5000")
    return conn


def init(conn: sqlite3.Connection) -> None:
    conn.executescript(SCHEMA)


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
