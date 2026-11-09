-- 026: a 热度快讯 is one item per episode, updated in place every hour until 24 hours after airing; `updated_at` is
-- the time of its latest figure (NULL: never updated).
ALTER TABLE news ADD COLUMN updated_at TEXT;
