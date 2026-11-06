-- 009: company capital policy (docs/v2-corporate.md §3): stock splits, rights issues, buybacks, and the price
-- adjustment factors they leave in the price history (前复权 K-lines). Cash moves through the ledger as usual
-- (rights, rights_funding, rights_comp, rights_short, production_fee; buybacks are ordinary trades).

-- one row per corporate action
CREATE TABLE IF NOT EXISTS corporate_actions (
    id          INTEGER PRIMARY KEY,
    company_id  INTEGER NOT NULL REFERENCES companies(id),
    kind        TEXT NOT NULL,                    -- split | rights | buyback
    announced   TEXT NOT NULL,                    -- trading day of the announcement (its daily close)
    effective   TEXT NOT NULL,                    -- split: first day in new units; rights: the record (and issue) day;
                                                  -- buyback: the last day of the programme
    status      TEXT NOT NULL,                    -- pending | active | done | cancelled
    params      TEXT NOT NULL                     -- JSON (crate::corporate::Params)
);
CREATE INDEX IF NOT EXISTS corporate_actions_company ON corporate_actions(company_id);

-- a player's subscription to a rights issue (cash frozen until the record day, like ipo_applications)
CREATE TABLE IF NOT EXISTS rights_subscriptions (
    action_id   INTEGER NOT NULL REFERENCES corporate_actions(id),
    account_id  INTEGER NOT NULL REFERENCES accounts(id),
    qty         INTEGER NOT NULL,
    frozen      INTEGER NOT NULL,                 -- cents
    allotted    INTEGER,                          -- NULL until the record day
    created_at  TEXT NOT NULL,
    PRIMARY KEY (action_id, account_id)
);

-- price history adjustment: prices before `day` × price_factor, volumes × qty_factor (前复权 composes them)
CREATE TABLE IF NOT EXISTS price_adjustments (
    company_id   INTEGER NOT NULL REFERENCES companies(id),
    day          TEXT NOT NULL,                   -- first trading day in the new terms (除权日)
    price_factor REAL NOT NULL,
    qty_factor   REAL NOT NULL,
    action_id    INTEGER NOT NULL,
    PRIMARY KEY (company_id, day, action_id)
);
