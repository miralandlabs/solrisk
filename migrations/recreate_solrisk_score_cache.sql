-- Destructive repair: drop and recreate solrisk_score_cache (canonical v2 DDL).
-- Safe on preview; only ephemeral cache rows are lost.
--
-- After a full recreate you do NOT need 003 or 004 — those are legacy-upgrade only.
-- RLS + backend policies are applied inline below.

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

ALTER TABLE solrisk_score_cache ENABLE ROW LEVEL SECURITY;

CREATE POLICY solrisk_score_cache_backend
    ON solrisk_score_cache
    FOR ALL
    USING (current_user NOT IN ('anon', 'authenticated'))
    WITH CHECK (current_user NOT IN ('anon', 'authenticated'));

CREATE POLICY solrisk_score_cache_service_role_all
    ON solrisk_score_cache
    FOR ALL
    TO service_role
    USING (true)
    WITH CHECK (true);

CREATE POLICY solrisk_score_cache_postgres_all
    ON solrisk_score_cache
    FOR ALL
    TO postgres
    USING (true)
    WITH CHECK (true);

GRANT ALL ON TABLE solrisk_score_cache TO postgres;
GRANT ALL ON TABLE solrisk_score_cache TO service_role;
