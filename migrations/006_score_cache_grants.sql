-- Table privileges for backend roles (RLS policies are in 005_score_cache_rls.sql).
-- DROP/CREATE drops grants — run after a manual recreate if you skipped recreate_solrisk_score_cache.sql.

GRANT ALL ON TABLE solrisk_score_cache TO postgres;
GRANT ALL ON TABLE solrisk_score_cache TO service_role;
