-- Fix score cache PK / subject column for v0.1 → v2 upgrades where
-- 002_parameters_v2.sql PK migration failed silently (EXCEPTION handler).
-- Idempotent. Safe on fresh init.sql installs.

-- Backfill subject from legacy wallet_pubkey column when present.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'solrisk_score_cache' AND column_name = 'wallet_pubkey'
    ) THEN
        UPDATE solrisk_score_cache
        SET subject = wallet_pubkey
        WHERE subject IS NULL OR subject = '';
    END IF;
END $$;

-- Ensure composite PK (endpoint, subject).
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.table_constraints
        WHERE table_name = 'solrisk_score_cache'
          AND constraint_type = 'PRIMARY KEY'
          AND constraint_name = 'solrisk_score_cache_pkey'
    ) THEN
        ALTER TABLE solrisk_score_cache ADD PRIMARY KEY (endpoint, subject);
    END IF;
EXCEPTION
    WHEN invalid_table_definition OR unique_violation OR not_null_violation THEN
        -- Drop legacy PK and retry once.
        ALTER TABLE solrisk_score_cache DROP CONSTRAINT IF EXISTS solrisk_score_cache_pkey;
        ALTER TABLE solrisk_score_cache ADD PRIMARY KEY (endpoint, subject);
END $$;
