-- 005: QQ bindings for the bot (docs/api-v1.md §3.5). The web page creates a one-time code for the
-- signed-in account; the bot redeems it for a QQ number. One account per QQ number and one QQ number per
-- account (per platform). Not game state: written by the API, not by the engine.

-- one-time codes (8 Crockford base32 characters, stored without the hyphen); 10 minutes; one live code
-- per account (a new one replaces the old)
CREATE TABLE IF NOT EXISTS bind_codes (
    code        TEXT PRIMARY KEY,
    account_id  INTEGER NOT NULL REFERENCES accounts(id),
    scopes      TEXT NOT NULL,          -- comma-separated, e.g. "read:account"
    created_at  TEXT NOT NULL,
    expires_at  TEXT NOT NULL,
    used_at     TEXT
);
CREATE INDEX IF NOT EXISTS bind_codes_account ON bind_codes(account_id);

CREATE TABLE IF NOT EXISTS bindings (
    id            INTEGER PRIMARY KEY,
    account_id    INTEGER NOT NULL REFERENCES accounts(id),
    platform      TEXT NOT NULL,        -- "qq"
    external_id   TEXT NOT NULL,        -- the QQ number
    scopes        TEXT NOT NULL,        -- comma-separated
    created_at    TEXT NOT NULL,
    last_used_at  TEXT,
    UNIQUE(platform, external_id),
    UNIQUE(account_id, platform)
);
