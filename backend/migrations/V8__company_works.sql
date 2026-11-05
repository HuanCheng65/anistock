-- 008 (phase 1): a company is a series (docs/v2-series.md §3): the works it owns besides its first one (the company
-- id is the AniList id of the series' first listed work, which needs no row). A new season of a listed series joins
-- its company here instead of listing; the company's revenue and value sum its works. Each work keeps its own
-- valuation state (valuation_joint is keyed by AniList id).
CREATE TABLE IF NOT EXISTS company_works (
    work_id     INTEGER PRIMARY KEY,           -- AniList id
    company_id  INTEGER NOT NULL REFERENCES companies(id),
    joined_on   TEXT NOT NULL                  -- trading day
);
CREATE INDEX IF NOT EXISTS company_works_company ON company_works(company_id);
