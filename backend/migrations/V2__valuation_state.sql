-- 002: the valuation filter's state (docs/v2-valuation.md §6): one row per show and market, updated at each
-- daily close and each 热度快讯; the hidden value is recomputed from it, so a restart loses nothing.
CREATE TABLE IF NOT EXISTS valuation_state (
    company_id  INTEGER NOT NULL,
    market      TEXT NOT NULL,             -- us | cn | jp
    mean        REAL NOT NULL,             -- log level
    var         REAL NOT NULL,             -- its variance
    last_obs    REAL,                      -- the last raw observation (cn/jp weekly series: update on change)
    acc         REAL NOT NULL DEFAULT 0,   -- us: sum of this episode week's daily log values so far
    acc_n       INTEGER NOT NULL DEFAULT 0,
    day         TEXT NOT NULL,             -- the day the state refers to
    updated_at  TEXT NOT NULL,
    PRIMARY KEY (company_id, market)
);
