CREATE TABLE IF NOT EXISTS translation_cache (
    cache_key   TEXT        NOT NULL PRIMARY KEY,  -- sha256(target_lang + ":" + text)
    target_lang TEXT        NOT NULL,
    source_text TEXT        NOT NULL,
    translated  TEXT        NOT NULL,
    cached_at   BIGINT      NOT NULL               -- Unix ms
);

CREATE INDEX IF NOT EXISTS idx_translation_cache_cached_at ON translation_cache (cached_at);
