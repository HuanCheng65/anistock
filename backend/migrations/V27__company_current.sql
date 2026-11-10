-- 027: a company's current work (docs/design/series.md §13.7): of the works it owns, the one on air, else the next
-- season once it has joined, else the latest; the market lists, the 本季新番 filter and index, 今晚放送 and the cover
-- follow it, not the work the company listed with. Written by the daily close.
CREATE TABLE IF NOT EXISTS company_current (
    company_id   INTEGER PRIMARY KEY REFERENCES companies(id),
    work_id      INTEGER NOT NULL,              -- AniList id
    first_air    TEXT NOT NULL,
    episodes     INTEGER,                       -- NULL: not announced
    aired        INTEGER NOT NULL,
    long_runner  INTEGER NOT NULL,
    since        TEXT NOT NULL                  -- the close that made it the current work
);
