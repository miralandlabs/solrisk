-- Backend-only cache: RLS on, policies for server roles (not anon/authenticated clients).
-- Idempotent. Safe after recreate_solrisk_score_cache.sql (policies included there too).

ALTER TABLE solrisk_score_cache ENABLE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS solrisk_score_cache_service_role_all ON solrisk_score_cache;
DROP POLICY IF EXISTS solrisk_score_cache_postgres_all ON solrisk_score_cache;
DROP POLICY IF EXISTS solrisk_score_cache_backend ON solrisk_score_cache;

-- Covers postgres, service_role, and any other non-client Supabase/Vercel DB role.
CREATE POLICY solrisk_score_cache_backend
    ON solrisk_score_cache
    FOR ALL
    USING (current_user NOT IN ('anon', 'authenticated'))
    WITH CHECK (current_user NOT IN ('anon', 'authenticated'));

-- Explicit role policies (harmless if redundant with backend policy above).
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
