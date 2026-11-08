-- 指数基金 (docs/v3-funds.md §3): what a theme index selects on, refreshed by every close from the inputs.
-- source: AniList source (MANGA, LIGHT_NOVEL, ORIGINAL, ...); genres: AniList genres plus the theme tags (Isekai),
-- comma-separated.
ALTER TABLE companies ADD COLUMN source TEXT;
ALTER TABLE companies ADD COLUMN genres TEXT NOT NULL DEFAULT '';
