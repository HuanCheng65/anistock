-- 019: 热度期货 (docs/v3-futures.md, docs/v3-futures-impl.md). Separate from the stock tables: a futures order
-- freezes margin instead of cash or shares, a position is signed and has no lots, and no notional changes hands
-- at a fill. Prices are in hundredths of an index point (one tick); a contract's tick_value says what one tick of
-- one lot is worth in cents. Variation margin moves through the ledger at the daily close (futures_mtm, through
-- the 'clearing' account), so the ledger keeps summing to zero.

-- the published heat index, frozen in the game database once a close has read it (settlement can be re-derived
-- from game.db alone)
CREATE TABLE IF NOT EXISTS heat_index (
    work_id     INTEGER NOT NULL,
    day         TEXT NOT NULL,
    idx         REAL NOT NULL,
    us          REAL, cn REAL, jp REAL,
    flags       TEXT NOT NULL DEFAULT '{}',   -- JSON: market -> missing | filled | negative | spike
    version     TEXT NOT NULL,
    PRIMARY KEY (work_id, day)
);

CREATE TABLE IF NOT EXISTS futures_contracts (
    id            INTEGER PRIMARY KEY,
    code          TEXT NOT NULL UNIQUE,
    company_id    INTEGER NOT NULL REFERENCES companies(id),
    work_id       INTEGER NOT NULL,             -- the AniList work whose index it settles on
    period        INTEGER NOT NULL,             -- k: weeks 4k-3 .. 4k after the premiere
    first_day     TEXT NOT NULL,                -- first and last day averaged
    last_day      TEXT NOT NULL,
    listed_on     TEXT NOT NULL,
    status        TEXT NOT NULL DEFAULT 'listed',  -- listed | expiring (waiting for the index) | settled
    tick_value    INTEGER NOT NULL,             -- cents per tick per lot
    forecast      INTEGER NOT NULL,             -- the model's forecast of the average when listed (ticks)
    prev_settle   INTEGER NOT NULL,             -- last settlement price (the VCM's previous close)
    open_ref      INTEGER,
    vcm_until     TEXT,
    vcm_low       INTEGER,
    vcm_high      INTEGER,
    vcm_count     INTEGER NOT NULL DEFAULT 0,
    last_price    INTEGER NOT NULL,
    final_price   INTEGER,
    settled_on    TEXT
);

CREATE TABLE IF NOT EXISTS futures_orders (
    id           INTEGER PRIMARY KEY,
    account_id   INTEGER NOT NULL REFERENCES accounts(id),
    contract_id  INTEGER NOT NULL REFERENCES futures_contracts(id),
    side         TEXT NOT NULL,                 -- buy | sell
    kind         TEXT NOT NULL,                 -- limit | market
    price        INTEGER,
    qty          INTEGER NOT NULL,
    filled       INTEGER NOT NULL DEFAULT 0,
    frozen       INTEGER NOT NULL DEFAULT 0,    -- cents frozen: initial margin for the opening part, and commission
    status       TEXT NOT NULL,                 -- open | filled | cancelled
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS futures_orders_open ON futures_orders(contract_id, status);
CREATE INDEX IF NOT EXISTS futures_orders_account ON futures_orders(account_id, status);

CREATE TABLE IF NOT EXISTS futures_trades (
    id           INTEGER PRIMARY KEY,
    contract_id  INTEGER NOT NULL REFERENCES futures_contracts(id),
    price        INTEGER NOT NULL,
    qty          INTEGER NOT NULL,
    buyer_id     INTEGER NOT NULL,
    seller_id    INTEGER NOT NULL,
    buy_order    INTEGER,
    sell_order   INTEGER,
    ts           TEXT NOT NULL,
    day          TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS futures_trades_contract ON futures_trades(contract_id, day);

-- net position per account and contract: qty > 0 long, < 0 short. basis = the signed ticks paid since the last
-- settlement (the settlement price for what was held then, the trade price for what traded since), so the
-- variation margin at settlement price S is (qty * S - basis) * tick_value, and over all accounts it sums to zero.
-- margin: cents of initial margin held for it (part of accounts.cash_frozen).
CREATE TABLE IF NOT EXISTS futures_positions (
    account_id   INTEGER NOT NULL REFERENCES accounts(id),
    contract_id  INTEGER NOT NULL REFERENCES futures_contracts(id),
    qty          INTEGER NOT NULL,
    basis        INTEGER NOT NULL,
    margin       INTEGER NOT NULL DEFAULT 0,
    cost         INTEGER NOT NULL DEFAULT 0,  -- signed ticks paid for the open lots at entry (average cost)
    PRIMARY KEY (account_id, contract_id)
);

-- one row per contract and trading day: the day's trading and its settlement
CREATE TABLE IF NOT EXISTS futures_days (
    contract_id    INTEGER NOT NULL REFERENCES futures_contracts(id),
    day            TEXT NOT NULL,
    open           INTEGER, high INTEGER, low INTEGER, close INTEGER,
    volume         INTEGER NOT NULL DEFAULT 0,
    settle         INTEGER NOT NULL,
    settle_source  TEXT NOT NULL,              -- vwap | mid | prev | final
    open_interest  INTEGER NOT NULL,
    PRIMARY KEY (contract_id, day)
);
