-- 008: margin trading and short selling (融资融券, docs/v2-margin.md). The player's own account is the credit
-- account; these tables hold what it owes. Cash moves through the ledger as usual (margin_loan, margin_repay,
-- margin_interest, lending_fee, short_dividend, short_settle), so the ledger still sums to zero.

-- one row per account that has opened credit
CREATE TABLE IF NOT EXISTS credit_accounts (
    account_id  INTEGER PRIMARY KEY REFERENCES accounts(id),
    opened_at   TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'normal',   -- normal | call | liquidating | defaulted
    call_day    TEXT,                             -- trading day of the margin call (status 'call')
    bad_debt    INTEGER NOT NULL DEFAULT 0        -- cents left unpaid after a default (穿仓), written off on restructuring
);

-- financing owed, per account and stock (the stock it was drawn to buy)
CREATE TABLE IF NOT EXISTS margin_debts (
    account_id  INTEGER NOT NULL REFERENCES accounts(id),
    company_id  INTEGER NOT NULL REFERENCES companies(id),
    principal   INTEGER NOT NULL,                 -- cents
    interest    INTEGER NOT NULL DEFAULT 0,       -- cents accrued and not yet paid
    opened_on   TEXT NOT NULL,
    PRIMARY KEY (account_id, company_id)
);

-- shares owed, per account and stock
CREATE TABLE IF NOT EXISTS short_positions (
    account_id  INTEGER NOT NULL REFERENCES accounts(id),
    company_id  INTEGER NOT NULL REFERENCES companies(id),
    lender_id   INTEGER NOT NULL REFERENCES accounts(id),
    qty         INTEGER NOT NULL,                 -- shares borrowed and not yet returned
    proceeds    INTEGER NOT NULL DEFAULT 0,       -- cents of short-sale proceeds locked (part of accounts.cash_frozen)
    fee         INTEGER NOT NULL DEFAULT 0,       -- cents of lending fees / manufactured dividends not yet paid
    opened_on   TEXT NOT NULL,
    PRIMARY KEY (account_id, company_id)
);

-- the credit mode of an order (orders without a row are ordinary)
CREATE TABLE IF NOT EXISTS margin_orders (
    order_id     INTEGER PRIMARY KEY REFERENCES orders(id),
    mode         TEXT NOT NULL,                   -- margin_buy | short_sell | sell_repay | buy_return
    from_locked  INTEGER NOT NULL DEFAULT 0       -- buy_return: cents of locked proceeds moved into the order
);

ALTER TABLE player_days ADD COLUMN liabilities INTEGER NOT NULL DEFAULT 0;
