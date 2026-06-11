//! Subscription tiers, JWT issue/verify (HS256).

use chrono::{DateTime, Utc};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::pricing::{ALL_TIERS, TIER_DAILY, TIER_HOURLY, TIER_MONTHLY};

pub const JWT_PERSISTENCE_HINT: &str =
    "Save the token locally until expiresAt; reuse Authorization: Bearer on all data routes.";

pub const TIER_DURATIONS_SEC: &[(&str, i64)] = &[
    (TIER_HOURLY, 60 * 60),
    (TIER_DAILY, 24 * 60 * 60),
    (TIER_MONTHLY, 30 * 24 * 60 * 60),
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionClaims {
    pub payer: String,
    pub tier: String,
    pub sub: String,
    pub iat: i64,
    pub exp: i64,
}

pub fn tier_duration_secs(tier: &str) -> Option<i64> {
    TIER_DURATIONS_SEC
        .iter()
        .find(|(t, _)| *t == tier)
        .map(|(_, d)| *d)
}

pub fn tier_label(tier: &str) -> &'static str {
    match tier {
        TIER_HOURLY => "1 hour",
        TIER_DAILY => "24 hours",
        TIER_MONTHLY => "30 days",
        _ => "unknown",
    }
}

pub fn is_valid_tier(tier: &str) -> bool {
    ALL_TIERS.contains(&tier)
}

fn jwt_secret() -> Result<String, Error> {
    std::env::var("JWT_SECRET")
        .map_err(|_| Error::Internal("JWT_SECRET not set (required for subscription)".into()))
}

pub fn sign_subscription_token(
    payer: &str,
    tier: &str,
    issued_at: DateTime<Utc>,
) -> Result<(String, DateTime<Utc>), Error> {
    let dur =
        tier_duration_secs(tier).ok_or_else(|| Error::Internal(format!("unknown tier: {tier}")))?;
    let exp = issued_at + chrono::Duration::seconds(dur);
    let claims = SubscriptionClaims {
        payer: payer.to_string(),
        tier: tier.to_string(),
        sub: "solrisk".to_string(),
        iat: issued_at.timestamp(),
        exp: exp.timestamp(),
    };
    let secret = jwt_secret()?;
    let token = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| Error::Internal(format!("jwt sign: {e}")))?;
    Ok((token, exp))
}

pub fn verify_subscription_token(token: &str) -> Result<SubscriptionClaims, Error> {
    let secret = jwt_secret()?;
    let data = decode::<SubscriptionClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::new(Algorithm::HS256),
    )
    .map_err(|e| Error::Internal(format!("jwt verify: {e}")))?;
    Ok(data.claims)
}

pub fn rate_limit_per_payer_per_min() -> u32 {
    std::env::var("RATE_LIMIT_PER_PAYER_PER_MIN")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60)
}

pub fn rate_limit_global_per_min() -> u32 {
    std::env::var("RATE_LIMIT_GLOBAL_PER_MIN")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(200)
}
