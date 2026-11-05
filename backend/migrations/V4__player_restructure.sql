-- 004: voluntary restructuring (破产重整, docs/game-design.md). A player whose total assets fall under
-- game.restructure_threshold of the starting cash may, once per season, sell every holding to the world
-- account at the last price and have cash topped up to the starting amount. The 总榜 return stays
-- time-weighted across it: return = return_factor * (assets / segment_base) - 1.

-- one row per restructuring
CREATE TABLE IF NOT EXISTS restructures (
    id             INTEGER PRIMARY KEY,
    account_id     INTEGER NOT NULL REFERENCES accounts(id),
    day            TEXT NOT NULL,          -- trading day
    ts             TEXT NOT NULL,
    assets_before  INTEGER NOT NULL,       -- cents: cash (frozen included) + holdings at last price
    sold           INTEGER NOT NULL,       -- cents paid by the world account for the holdings
    injected       INTEGER NOT NULL        -- cents paid by the world account to top cash up
);
CREATE INDEX IF NOT EXISTS restructures_account ON restructures(account_id, day);

-- the chain-linked return of a player; no row = factor 1, base = game.initial_cash, never restructured
CREATE TABLE IF NOT EXISTS player_returns (
    account_id     INTEGER PRIMARY KEY REFERENCES accounts(id),
    return_factor  REAL NOT NULL,          -- product of closed segments' end / start
    segment_base   INTEGER NOT NULL,       -- cents: the current segment's starting assets
    restructures   INTEGER NOT NULL,       -- count
    last_day       TEXT NOT NULL           -- trading day of the latest restructuring
);
