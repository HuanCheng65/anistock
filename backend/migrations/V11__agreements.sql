-- 011: signed agreements (docs/v2-account.md §7, §10). The user agreement and the privacy policy are
-- accepted at registration; the risk disclosure is signed (the username typed as a signature) in the
-- account-opening flow. A new version of a document asks for a new row. Not game state: written by the
-- API, not by the engine.
CREATE TABLE IF NOT EXISTS account_agreements (
    account_id  INTEGER NOT NULL REFERENCES accounts(id),
    doc         TEXT NOT NULL,          -- "terms", "privacy", "disclosure"
    version     INTEGER NOT NULL,
    signed_at   TEXT NOT NULL,
    signature   TEXT,                   -- what was typed to sign (the disclosure); NULL for a checkbox
    PRIMARY KEY (account_id, doc, version)
);
