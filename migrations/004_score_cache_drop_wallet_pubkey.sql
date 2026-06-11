-- Legacy v0.1 score_cache kept wallet_pubkey NOT NULL; v2 INSERTs only set
-- (endpoint, subject). Drops the orphan column so cache writes succeed.
-- Safe on fresh init.sql installs (column never existed).

ALTER TABLE solrisk_score_cache DROP COLUMN IF EXISTS wallet_pubkey;
