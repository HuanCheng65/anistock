-- 012: 风险测评, the 融资融券 knowledge test, and sign-in records (docs/v2-account.md §4.1, §5, §6, §10).
-- Not game state: written by the API, not by the engine.
CREATE TABLE IF NOT EXISTS risk_assessments (
    id          INTEGER PRIMARY KEY,
    account_id  INTEGER NOT NULL REFERENCES accounts(id),
    version     INTEGER NOT NULL,       -- of the questionnaire
    answers     TEXT NOT NULL,          -- JSON array of option indexes
    score       INTEGER NOT NULL,
    level       INTEGER NOT NULL,       -- 1..5 = C1..C5
    taken_at    TEXT NOT NULL,
    day         TEXT NOT NULL           -- trading day, for the per-day limit
);
CREATE INDEX IF NOT EXISTS risk_assessments_account ON risk_assessments(account_id, id);

CREATE TABLE IF NOT EXISTS quiz_results (
    id          INTEGER PRIMARY KEY,
    account_id  INTEGER NOT NULL REFERENCES accounts(id),
    quiz        TEXT NOT NULL,          -- "margin"
    version     INTEGER NOT NULL,
    score       INTEGER NOT NULL,
    passed      INTEGER NOT NULL,
    taken_at    TEXT NOT NULL,
    day         TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS quiz_results_account ON quiz_results(account_id, quiz, id);

-- one row per sign-in or registration; kept 6 months (网络安全法 §21); read by the wash-trade report
CREATE TABLE IF NOT EXISTS account_logins (
    account_id  INTEGER NOT NULL REFERENCES accounts(id),
    ip          TEXT NOT NULL,          -- the limit key: IPv4, or the IPv6 /64
    at          TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS account_logins_ip ON account_logins(ip, at);
CREATE INDEX IF NOT EXISTS account_logins_at ON account_logins(at);
