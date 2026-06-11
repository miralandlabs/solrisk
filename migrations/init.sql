-- solrisk v2 — complete schema (fresh install)
--
-- Use this file when provisioning a new database (or solrisk-only Supabase project).
-- All solrisk-specific tables use the `solrisk_` prefix.
-- The shared `parameters` table uses (service, endpoint, param_name) — same shape as
-- spl-token-balance / mintforge.
--
-- After this file, run ONE pricing seed:
--   Preview/devnet:  parameters-seed-devnet.sql
--   Mainnet:         parameters-seed-mainnet.sql
--
-- Upgrading an existing v0.1 database (legacy parameters, wallet-only cache PK)?
-- See CUTOVER.md — use 002_parameters_v2.sql instead of re-running this file.

-- ============================================================================
-- parameters (v2 — multi-tenant, per-endpoint pricing)
-- ============================================================================
CREATE TABLE IF NOT EXISTS parameters (
    id             BIGSERIAL PRIMARY KEY,
    service        TEXT NOT NULL DEFAULT 'solrisk',
    endpoint       TEXT NOT NULL DEFAULT '*',
    param_name     TEXT NOT NULL,
    param_value    TEXT NOT NULL,
    inactive       BOOLEAN NOT NULL DEFAULT FALSE,
    effective_from TIMESTAMPTZ,
    expires_at     TIMESTAMPTZ,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS uniq_parameters_service_endpoint_param
    ON parameters (service, endpoint, param_name);
CREATE INDEX IF NOT EXISTS idx_parameters_service_active
    ON parameters (service, inactive);

-- ============================================================================
-- solrisk_wallet_labels — curated allow/deny labels (DB overrides JSONL)
-- Sources: 'ofac', 'chainabuse', 'internal', 'jupiter', 'user_report'
-- weight > 0 = deny penalty; weight < 0 = allow bonus
-- ============================================================================
CREATE TABLE IF NOT EXISTS solrisk_wallet_labels (
    wallet_pubkey  TEXT NOT NULL,
    source         TEXT NOT NULL,
    label          TEXT NOT NULL,
    weight         INTEGER NOT NULL DEFAULT 0,
    evidence_url   TEXT,
    added_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (wallet_pubkey, source, label)
);

CREATE INDEX IF NOT EXISTS idx_solrisk_labels_wallet
    ON solrisk_wallet_labels (wallet_pubkey);

-- ============================================================================
-- solrisk_scoring_log — audit trail (billing reconciliation + model tuning)
-- ============================================================================
CREATE TABLE IF NOT EXISTS solrisk_scoring_log (
    id              BIGSERIAL PRIMARY KEY,
    wallet_pubkey   TEXT NOT NULL,
    endpoint        TEXT NOT NULL,
    subject         TEXT NOT NULL,
    score           INTEGER NOT NULL,
    band            TEXT NOT NULL,
    scoring_version TEXT NOT NULL,
    signals_json    JSONB,
    payer           TEXT,
    correlation_id  TEXT,
    settlement_sig  TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_solrisk_scoring_log_subject
    ON solrisk_scoring_log (endpoint, subject, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_solrisk_scoring_log_payer
    ON solrisk_scoring_log (payer, created_at DESC)
    WHERE payer IS NOT NULL;

-- ============================================================================
-- solrisk_scam_reports — user-submitted reports (moderated → labels)
-- ============================================================================
CREATE TABLE IF NOT EXISTS solrisk_scam_reports (
    id               BIGSERIAL PRIMARY KEY,
    reported_wallet  TEXT NOT NULL,
    reporter_wallet  TEXT,
    reason           TEXT NOT NULL,
    evidence_url     TEXT,
    status           TEXT NOT NULL DEFAULT 'pending',
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_solrisk_scam_reports_wallet
    ON solrisk_scam_reports (reported_wallet);

-- ============================================================================
-- solrisk_score_cache — short-lived response cache (5 min TTL, app-enforced)
-- Keyed by (endpoint, subject) for wallet / token / tx routes
-- ============================================================================
CREATE TABLE IF NOT EXISTS solrisk_score_cache (
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

CREATE INDEX IF NOT EXISTS idx_solrisk_score_cache_expires
    ON solrisk_score_cache (expires_at);

-- ============================================================================
-- solrisk_subscriptions — issued JWTs (revocation by payer + issued_at)
-- ============================================================================
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

-- ============================================================================
-- solrisk_rate_buckets — serverless-safe per-minute rate limits
-- bucket_key: 'ip:<addr>' (global) or 'payer:<wallet>' (subscriber fair use)
-- ============================================================================
CREATE TABLE IF NOT EXISTS solrisk_rate_buckets (
    bucket_key    TEXT NOT NULL,
    window_start  TIMESTAMPTZ NOT NULL,
    count         INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (bucket_key, window_start)
);

-- ============================================================================
-- Next step: seed per-endpoint x402 pricing (pick one cluster)
-- ============================================================================
-- \i parameters-seed-devnet.sql
-- \i parameters-seed-mainnet.sql
