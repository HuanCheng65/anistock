-- 006: every player's assets after each daily close (docs/api-v1.md §4.5.10): the asset curve and the
-- 周榜 / 季榜. Written by the daily close, in the close's own transaction; one row per player per closed
-- trading day. The history before this migration is backfilled once at startup (store::player_days),
-- or, when the replay does not reproduce the current accounts, starts at the next close.
CREATE TABLE IF NOT EXISTS player_days (
    account_id    INTEGER NOT NULL REFERENCES accounts(id),
    day           TEXT NOT NULL,          -- trading day
    cash          INTEGER NOT NULL,       -- cents, after the close (frozen included)
    stock_value   INTEGER NOT NULL,       -- cents: holdings at the day's close prices
    return_index  REAL NOT NULL,          -- chain-linked, 1.0 at registration
    restructured  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (account_id, day)
);
CREATE INDEX IF NOT EXISTS player_days_day ON player_days(day);
