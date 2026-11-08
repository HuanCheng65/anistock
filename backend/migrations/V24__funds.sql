-- 指数基金 (docs/design/funds.md): the funds, their holders (units in hundredths, with the lots by the day they were
-- confirmed: the redemption fee), the requests, what a fund owes redeeming holders, the daily net value, and the
-- month's choices (the index float, 安易 20's members) as JSON.
CREATE TABLE IF NOT EXISTS funds (
    key            TEXT PRIMARY KEY,
    account_id     INTEGER NOT NULL,
    units          INTEGER NOT NULL,
    pending_cash   INTEGER NOT NULL,
    nav            REAL NOT NULL,
    own_costs      INTEGER NOT NULL DEFAULT 0,
    impact_charged INTEGER NOT NULL DEFAULT 0,
    fees_paid      INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS fund_holders (
    fund       TEXT NOT NULL,
    account_id INTEGER NOT NULL,
    units      INTEGER NOT NULL,
    lots       TEXT NOT NULL,
    PRIMARY KEY (fund, account_id)
);
CREATE INDEX IF NOT EXISTS fund_holders_account ON fund_holders(account_id);
CREATE TABLE IF NOT EXISTS fund_requests (
    id           INTEGER PRIMARY KEY,
    fund         TEXT NOT NULL,
    account_id   INTEGER NOT NULL,
    kind         TEXT NOT NULL,              -- subscribe | redeem
    amount       INTEGER NOT NULL,           -- cents subscribed | units (hundredths) redeemed
    day          TEXT NOT NULL,              -- the trading day it was submitted on
    young        INTEGER NOT NULL DEFAULT 0, -- redeemed units held under the fee's days
    status       TEXT NOT NULL,              -- pending | executing | confirmed | cancelled
    created_at   TEXT NOT NULL,
    taken        TEXT NOT NULL DEFAULT '[]',
    confirmed_on TEXT,
    nav          REAL,
    units        INTEGER,
    cash         INTEGER,
    cost         INTEGER
);
CREATE INDEX IF NOT EXISTS fund_requests_account ON fund_requests(account_id, id);
CREATE TABLE IF NOT EXISTS fund_owed (
    fund       TEXT NOT NULL,
    account_id INTEGER NOT NULL,
    cents      INTEGER NOT NULL,
    PRIMARY KEY (fund, account_id)
);
CREATE TABLE IF NOT EXISTS fund_nav (
    fund  TEXT NOT NULL,
    day   TEXT NOT NULL,
    nav   REAL NOT NULL,
    units INTEGER NOT NULL,
    aum   INTEGER NOT NULL,
    cash  INTEGER NOT NULL,
    fee   INTEGER NOT NULL,
    PRIMARY KEY (fund, day)
);
CREATE TABLE IF NOT EXISTS fund_marks (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
