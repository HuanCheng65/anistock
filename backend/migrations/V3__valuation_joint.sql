-- 003 (v0.3.2): the joint valuation filter (docs/v2-valuation.md §3.2): one row per show with the three markets'
-- log levels, their covariance and the bookkeeping, as JSON (crate::vclose::JState). Replaces valuation_state,
-- whose rows are converted at load when this table is empty (diagonal covariance); the go-live rebuild
-- (ANISTOCK_VALUATION_REBUILD=1) replays the inputs history instead.
CREATE TABLE IF NOT EXISTS valuation_joint (
    company_id  INTEGER PRIMARY KEY,
    state       TEXT NOT NULL,             -- JSON: mean[3], cov[3][3], active[3], seen[3], last_obs[3], zero_days[3], acc, acc_n
    day         TEXT NOT NULL,             -- the day the state refers to
    updated_at  TEXT NOT NULL
);
