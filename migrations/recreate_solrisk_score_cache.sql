-- Destructive repair: drop and recreate solrisk_score_cache.
-- DDL is identical to migrations/init.sql (canonical v2 schema).
-- Safe on preview; only ephemeral cache rows are lost.

DROP TABLE IF EXISTS solrisk_score_cache;

CREATE TABLE solrisk_score_cache (
    endpoint        TEXT NOT NULL,
    subject         TEXT NOT NULL,
    score           INTEGER NOT NULL,
    band            TEXT NOT NULL,
    response_json   JSONB NOT NULL,
    scoring_version TEXT NOT NULL,
    cached_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at      TIMESTAMPTZ NOT NULL DEFAULT NOW() + INTERVAL '5 minutes',
    PRIMARY KEY (endpoint, subject)
);

CREATE INDEX idx_solrisk_score_cache_expires
    ON solrisk_score_cache (expires_at);
