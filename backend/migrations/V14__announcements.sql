-- 014: operator announcements shown once to each player as a popup after sign-in (`anistock announce`). Side
-- tables like account_agreements: the API reads and writes them directly, the engine does not hold them, so an
-- announcement can be posted while the server runs.
CREATE TABLE IF NOT EXISTS announcements (
    id          INTEGER PRIMARY KEY,
    created_at  TEXT NOT NULL,                 -- timestamp (+08:00)
    title       TEXT NOT NULL,
    body        TEXT NOT NULL,                 -- plain text; blank lines separate paragraphs
    popup       INTEGER NOT NULL DEFAULT 1     -- 1: shown as a popup to players who have not read it
);

-- who has dismissed which announcement
CREATE TABLE IF NOT EXISTS announcement_reads (
    account_id      INTEGER NOT NULL REFERENCES accounts(id),
    announcement_id INTEGER NOT NULL REFERENCES announcements(id),
    read_at         TEXT NOT NULL,
    PRIMARY KEY (account_id, announcement_id)
);
