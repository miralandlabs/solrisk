-- solrisk: Solana wallet risk scoring service
-- Shares the same Supabase DB as spl-token-balance-serverless.
-- All solrisk-specific tables use `solrisk_` prefix to avoid collision.
-- The shared `parameters` table uses `SOLRISK_` param_name prefix.

-- ============================================================================
-- Shared table: parameters (same schema as spl-token-balance-serverless)
-- If this table already exists from spl-balance, this is a no-op.
-- ============================================================================
CREATE TABLE IF NOT EXISTS parameters (
    id             BIGSERIAL PRIMARY KEY,
    param_name     TEXT NOT NULL,
    param_value    TEXT NOT NULL,
    inactive       BOOLEAN NOT NULL DEFAULT FALSE,
    effective_from TIMESTAMPTZ,
    expires_at     TIMESTAMPTZ,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS uniq_parameters_param_name ON parameters (param_name ASC);

-- ============================================================================
-- solrisk-specific tables
-- ============================================================================

-- Curated wallet labels (allow/deny). One row per (wallet, source, label).
-- Sources: 'ofac', 'chainabuse', 'internal', 'jupiter', 'user_report'
CREATE TABLE IF NOT EXISTS solrisk_wallet_labels (
    wallet_pubkey  TEXT NOT NULL,
    source         TEXT NOT NULL,
    label          TEXT NOT NULL,
    weight         INTEGER NOT NULL DEFAULT 0,
    evidence_url   TEXT,
    added_at       TIMESTAMPTZ DEFAULT NOW(),
    PRIMARY KEY (wallet_pubkey, source, label)
);

CREATE INDEX IF NOT EXISTS idx_solrisk_labels_wallet
    ON solrisk_wallet_labels (wallet_pubkey);

-- Audit trail of scored requests (billing reconciliation + model improvement).
CREATE TABLE IF NOT EXISTS solrisk_scoring_log (
    id              BIGSERIAL PRIMARY KEY,
    wallet_pubkey   TEXT NOT NULL,
    score           INTEGER NOT NULL,
    band            TEXT NOT NULL,
    scoring_version TEXT NOT NULL,
    signals_json    JSONB,
    payer           TEXT,
    correlation_id  TEXT,
    created_at      TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_solrisk_scoring_log_wallet
    ON solrisk_scoring_log (wallet_pubkey, created_at DESC);

-- User-submitted scam reports, moderated before promotion to labels.
CREATE TABLE IF NOT EXISTS solrisk_scam_reports (
    id               BIGSERIAL PRIMARY KEY,
    reported_wallet  TEXT NOT NULL,
    reporter_wallet  TEXT,
    reason           TEXT NOT NULL,
    evidence_url     TEXT,
    status           TEXT NOT NULL DEFAULT 'pending',
    created_at       TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_solrisk_scam_reports_wallet
    ON solrisk_scam_reports (reported_wallet);

-- Short-lived score cache (avoids redundant RPC fan-out for hot wallets).
-- TTL enforced application-side; expired rows cleaned by periodic cron.
CREATE TABLE IF NOT EXISTS solrisk_score_cache (
    wallet_pubkey   TEXT PRIMARY KEY,
    score           INTEGER NOT NULL,
    band            TEXT NOT NULL,
    response_json   JSONB NOT NULL,
    scoring_version TEXT NOT NULL,
    cached_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at      TIMESTAMPTZ NOT NULL DEFAULT NOW() + INTERVAL '5 minutes'
);

-- Example solrisk parameters (uncomment / adjust):
-- INSERT INTO parameters (param_name, param_value) VALUES
--   ('X402_NETWORK', 'solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1'),
--   ('X402_PAY_TO', '<YourSolriskVaultPDA>'),
--   ('MERCHANT_WALLET', '<YourSolriskSellerWallet>'),
--   (
--     'X402_ACCEPTS_JSON',
--     '[{"kind":"usdc","amountUi":"0.005"}]'
--   )
-- ON CONFLICT (param_name) DO UPDATE SET
--   param_value = EXCLUDED.param_value,
--   updated_at = NOW();
