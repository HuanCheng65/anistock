-- 013: changing and resetting passwords (docs/v2-account.md §13). Not game state.
-- One-time reset codes: issued by the QQ bot to the account bound to the asking QQ (or by the operator
-- with `anistock reset-code`); only the SHA-256 of the code is kept; 10 minutes; 5 wrong tries spend it.
CREATE TABLE IF NOT EXISTS password_resets (
    code_hash   TEXT PRIMARY KEY,
    account_id  INTEGER NOT NULL REFERENCES accounts(id),
    source      TEXT NOT NULL,          -- "qq" | "operator"
    created_at  TEXT NOT NULL,
    expires_at  TEXT NOT NULL,
    used_at     TEXT,
    attempts    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS password_resets_account ON password_resets(account_id);

-- when the password last changed (Unix seconds): session cookies issued before it no longer count
CREATE TABLE IF NOT EXISTS account_security (
    account_id     INTEGER PRIMARY KEY REFERENCES accounts(id),
    pw_changed_at  INTEGER NOT NULL
);
