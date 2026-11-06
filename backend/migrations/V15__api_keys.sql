-- 015: personal API keys for scripts (docs/v2-api-access.md). Not game state.
-- Only the SHA-256 of a key is kept; `prefix` (`anx_` + 4 characters) tells keys apart in the list.
CREATE TABLE IF NOT EXISTS api_keys (
    id            INTEGER PRIMARY KEY,
    account_id    INTEGER NOT NULL REFERENCES accounts(id),
    name          TEXT NOT NULL,
    prefix        TEXT NOT NULL,
    key_hash      BLOB NOT NULL UNIQUE,
    scopes        TEXT NOT NULL,          -- comma-separated: "read:account" (phase 1), later "trade"
    created_at    TEXT NOT NULL,
    expires_at    TEXT NOT NULL,
    last_used_at  TEXT,
    last_used_ip  TEXT,
    revoked_at    TEXT
);
CREATE INDEX IF NOT EXISTS api_keys_account ON api_keys(account_id);
