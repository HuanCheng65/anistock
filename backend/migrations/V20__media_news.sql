-- External news pushed by the news-feed stock extension (docs/v3-news.md): every judged event, archived for
-- calibration whatever `[news] mode` is. (V18 is api_orders; V19 is taken by the futures tables.)
CREATE TABLE IF NOT EXISTS media_news (
    id              INTEGER PRIMARY KEY,
    event_id        TEXT NOT NULL UNIQUE,      -- news-feed event id
    merged_into     TEXT,                      -- set when news-feed merged this event into another
    company_id      INTEGER NOT NULL,
    source          TEXT NOT NULL,
    url             TEXT NOT NULL,
    title           TEXT NOT NULL,             -- Chinese headline
    original_title  TEXT NOT NULL,
    published_at    TEXT,                      -- by the source
    fetched_at      TEXT NOT NULL,             -- by news-feed
    received_at     TEXT NOT NULL,             -- by the game (first push)
    updated_at      TEXT NOT NULL,
    event           TEXT NOT NULL,             -- sequel, movie, delay, hiatus, sales, award, collab, controversy, staff, rumor, denial, other
    direction       TEXT NOT NULL,             -- positive, negative, neutral
    magnitude       REAL NOT NULL,             -- 0-10
    credibility     TEXT NOT NULL,             -- official, media, rumor
    model           TEXT NOT NULL,
    probabilities   TEXT NOT NULL,             -- JSON: each question's full distribution
    status          TEXT NOT NULL,             -- stored, published, merged
    news_id         INTEGER,                   -- the game news item, once published
    label           TEXT                       -- manual label during calibration (JSON), NULL until labelled
);
CREATE INDEX IF NOT EXISTS media_news_company ON media_news(company_id, received_at);
