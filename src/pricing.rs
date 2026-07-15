//! Endpoint catalog — paths, tier keys, default pricing hints for SRM / gates.

pub const ENDPOINT_WALLET_RISK: &str = "wallet-risk";
pub const ENDPOINT_TOKEN_RISK: &str = "token-risk";
pub const ENDPOINT_TX_RISK: &str = "tx-risk";

pub const TIER_HOURLY: &str = "hourly";
pub const TIER_DAILY: &str = "daily";
pub const TIER_MONTHLY: &str = "monthly";

pub const ALL_TIERS: &[&str] = &[TIER_HOURLY, TIER_DAILY, TIER_MONTHLY];

/// Paid data routes. tx-risk is a billable SKU as of v0.3.0 (pre-sign screening).
pub const PER_CALL_ENDPOINTS: &[&str] =
    &[ENDPOINT_WALLET_RISK, ENDPOINT_TOKEN_RISK, ENDPOINT_TX_RISK];

/// Parameters table endpoint key for subscribe tiers (matches subscription-starter).
pub fn subscribe_endpoint_key(tier: &str) -> String {
    format!("/api/v1/subscribe/{tier}")
}

pub fn path_for_endpoint(endpoint: &str) -> Option<&'static str> {
    match endpoint {
        ENDPOINT_WALLET_RISK => Some("/api/v1/wallet-risk"),
        ENDPOINT_TOKEN_RISK => Some("/api/v1/token-risk"),
        ENDPOINT_TX_RISK => Some("/api/v1/tx-risk"),
        _ if endpoint.starts_with("/api/v1/subscribe/") => Some("/api/v1/subscribe"),
        _ => None,
    }
}

pub fn resource_description(endpoint: &str) -> &'static str {
    match endpoint {
        ENDPOINT_WALLET_RISK => "Solana wallet risk score",
        ENDPOINT_TOKEN_RISK => "Solana token rug-pull risk score",
        ENDPOINT_TX_RISK => "Solana transaction risk score",
        _ if endpoint.contains("subscribe/hourly") => "solrisk hourly subscription",
        _ if endpoint.contains("subscribe/daily") => "solrisk daily subscription",
        _ if endpoint.contains("subscribe/monthly") => "solrisk monthly subscription",
        _ => "solrisk paid endpoint",
    }
}

/// Sample query string for SRM probe URLs (no payment).
pub fn sample_query_for_endpoint(endpoint: &str) -> &'static str {
    match endpoint {
        ENDPOINT_WALLET_RISK => "wallet=11111111111111111111111111111112",
        ENDPOINT_TOKEN_RISK => "mint=So11111111111111111111111111111111111111112",
        // Format-valid base64 so the SRM probe reaches the 402 gate (full tx decode is post-payment).
        ENDPOINT_TX_RISK => "transaction=c29scmlzay1wcm9iZQ==",
        _ => "tier=hourly",
    }
}

pub fn default_legacy_usdc(endpoint: &str) -> f64 {
    // Fallback used only when neither the parameters table nor an env amount is set.
    // Keep in sync with migrations/parameters-seed-mainnet.sql.
    match endpoint {
        // tx-risk (pre-sign loss prevention) — the premium, value-dense SKU.
        ENDPOINT_TX_RISK => 0.30,
        // wallet-risk — real multi-hop fund-flow + counterparty AML (no longer commodity stats).
        ENDPOINT_WALLET_RISK => 0.25,
        // token-risk — held at beta pricing until P2 (LP/deployer depth).
        ENDPOINT_TOKEN_RISK => 0.10,
        _ if endpoint.contains("subscribe/monthly") => 25.0,
        _ if endpoint.contains("subscribe/daily") => 5.0,
        _ if endpoint.contains("subscribe/hourly") => 1.0,
        _ => 0.05,
    }
}
