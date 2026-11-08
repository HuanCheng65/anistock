-- 自选 on the server (it was per-device localStorage); newest first by rowid
CREATE TABLE IF NOT EXISTS watchlist (
    account_id  INTEGER NOT NULL REFERENCES accounts(id),
    company_id  INTEGER NOT NULL,
    added_at    TEXT NOT NULL,
    UNIQUE(account_id, company_id)
);
-- one-off onboarding marks: a page guide seen ('guide.stock', 'guide.order'), the 入门步骤 card closed
-- ('steps.closed'), the 首笔成交 note seen ('first_fill'), a 自选 added once ('watch')
CREATE TABLE IF NOT EXISTS account_marks (
    account_id  INTEGER NOT NULL REFERENCES accounts(id),
    key         TEXT NOT NULL,
    at          TEXT NOT NULL,
    PRIMARY KEY (account_id, key)
);
-- players who have traded already know the ropes: no guides, card or 首笔成交 note for them
INSERT OR IGNORE INTO account_marks(account_id, key, at)
SELECT a.id, k.key, strftime('%Y-%m-%dT%H:%M:%S', 'now')
FROM accounts a, (SELECT 'guide.stock' AS key UNION ALL SELECT 'guide.order' UNION ALL SELECT 'steps.closed' UNION ALL SELECT 'first_fill') k
WHERE a.kind = 'player' AND a.id IN (SELECT buyer_id FROM trades UNION SELECT seller_id FROM trades);
