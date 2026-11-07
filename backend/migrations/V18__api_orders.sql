-- Orders placed with an API key (docs/v2-api-access.md §6, §8.1): the 程序化 mark on the leaderboard and the
-- surveillance reports. Every other order came from the web. (V17 is taken by the futures tables.)
CREATE TABLE IF NOT EXISTS api_orders (
    order_id    INTEGER PRIMARY KEY,
    account_id  INTEGER NOT NULL,
    key_id      INTEGER NOT NULL,
    day         TEXT NOT NULL              -- trading day the order was placed
);
CREATE INDEX IF NOT EXISTS api_orders_account ON api_orders(account_id, day);
