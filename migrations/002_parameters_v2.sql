-- solrisk v2 UPGRADE migration (legacy v0.1 → v2)
-- Safe on shared Supabase with spl-token-balance / aethervane rows.
--
-- Use this when upgrading an EXISTING database that ran the old init.sql
-- (parameters without service/endpoint, wallet_pubkey-only score cache PK).
--
-- Fresh install? Run init.sql only — do NOT run this file.
-- Idempotent.

-- ---------------------------------------------------------------------------
-- parameters v2 columns (skip if spl-balance already migrated)
-- ---------------------------------------------------------------------------
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'parameters' AND column_name = 'service'
    ) THEN
        ALTER TABLE parameters ADD COLUMN service TEXT NOT NULL DEFAULT 'legacy';
        ALTER TABLE parameters ADD COLUMN endpoint TEXT NOT NULL DEFAULT '*';
    END IF;
END $$;

DROP INDEX IF EXISTS uniq_parameters_param_name;
CREATE UNIQUE INDEX IF NOT EXISTS uniq_parameters_service_endpoint_param
    ON parameters (service, endpoint, param_name);
CREATE INDEX IF NOT EXISTS idx_parameters_service_active
    ON parameters (service, inactive);

-- Backfill legacy solrisk rows (param_name prefixed or plain)
UPDATE parameters
SET service = 'solrisk', endpoint = '*'
WHERE service IN ('legacy', '') OR service IS NULL;

-- ---------------------------------------------------------------------------
-- subscriptions (JWT revocation)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS solrisk_subscriptions (
    id          BIGSERIAL PRIMARY KEY,
    payer       TEXT NOT NULL,
    tier        TEXT NOT NULL,
    issued_at   TIMESTAMPTZ NOT NULL,
    expires_at  TIMESTAMPTZ NOT NULL,
    tx_sig      TEXT,
    revoked     BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE INDEX IF NOT EXISTS idx_solrisk_subs_payer_issued
    ON solrisk_subscriptions (payer, issued_at);

-- ---------------------------------------------------------------------------
-- rate limit buckets (serverless-safe sliding window)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS solrisk_rate_buckets (
    bucket_key    TEXT NOT NULL,
    window_start  TIMESTAMPTZ NOT NULL,
    count         INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (bucket_key, window_start)
);

-- Extend scoring_log for multi-endpoint audit
ALTER TABLE solrisk_scoring_log
    ADD COLUMN IF NOT EXISTS endpoint TEXT,
    ADD COLUMN IF NOT EXISTS subject TEXT,
    ADD COLUMN IF NOT EXISTS settlement_sig TEXT;

-- Extend score cache for multi-endpoint
ALTER TABLE solrisk_score_cache
    ADD COLUMN IF NOT EXISTS endpoint TEXT NOT NULL DEFAULT 'wallet-risk',
    ADD COLUMN IF NOT EXISTS subject TEXT;

-- Migrate PK if old table exists with wallet_pubkey only
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.table_constraints
        WHERE table_name = 'solrisk_score_cache'
          AND constraint_type = 'PRIMARY KEY'
          AND constraint_name LIKE '%wallet%'
    ) THEN
        ALTER TABLE solrisk_score_cache DROP CONSTRAINT IF EXISTS solrisk_score_cache_pkey;
        ALTER TABLE solrisk_score_cache ADD PRIMARY KEY (endpoint, subject);
    ELSIF NOT EXISTS (
        SELECT 1 FROM information_schema.table_constraints
        WHERE table_name = 'solrisk_score_cache' AND constraint_type = 'PRIMARY KEY'
    ) THEN
        ALTER TABLE solrisk_score_cache ADD PRIMARY KEY (endpoint, subject);
    END IF;
EXCEPTION WHEN OTHERS THEN
    NULL;
END $$;
