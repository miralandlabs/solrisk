//! Endpoint catalog — paths, tier keys, default pricing hints for SRM / gates.

pub const ENDPOINT_WALLET_RISK: &str = "wallet-risk";
pub const ENDPOINT_TOKEN_RISK: &str = "token-risk";
pub const ENDPOINT_TX_RISK: &str = "tx-risk";

pub const TIER_HOURLY: &str = "hourly";
pub const TIER_DAILY: &str = "daily";
pub const TIER_MONTHLY: &str = "monthly";

pub const ALL_TIERS: &[&str] = &[TIER_HOURLY, TIER_DAILY, TIER_MONTHLY];

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
        ENDPOINT_TX_RISK => "signature=111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111",
        _ => "tier=hourly",
    }
}

pub fn default_legacy_usdc(endpoint: &str) -> f64 {
    match endpoint {
        ENDPOINT_TOKEN_RISK => 0.10,
        ENDPOINT_WALLET_RISK | ENDPOINT_TX_RISK => 0.05,
        _ if endpoint.contains("subscribe/monthly") => 25.0,
        _ if endpoint.contains("subscribe/daily") => 5.0,
        _ if endpoint.contains("subscribe/hourly") => 1.0,
        _ => 0.05,
    }
}
