//! DB-backed parameters with env fallback (pr402-style TTL cache).
//! DB is fully optional — when DATABASE_URL is unset, all config comes from
//! plain Vercel env vars (same pattern as spl-token-balance-serverless).
//!
//! If a DB is connected, param_name rows use `SOLRISK_` prefix to avoid
//! collision with other services sharing the same database.

use crate::db::ParametersDb;
use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant};
use tracing::warn;

pub struct ParametersCache {
    map: HashMap<String, String>,
    last_fetch: Option<Instant>,
}

impl ParametersCache {
    fn empty() -> Self {
        Self {
            map: HashMap::new(),
            last_fetch: None,
        }
    }
}

pub static PARAMETERS: OnceLock<RwLock<ParametersCache>> = OnceLock::new();

fn cache_store() -> &'static RwLock<ParametersCache> {
    PARAMETERS.get_or_init(|| RwLock::new(ParametersCache::empty()))
}

pub fn parameters_cache_ttl() -> Duration {
    static TTL: OnceLock<Duration> = OnceLock::new();
    *TTL.get_or_init(|| {
        std::env::var("SOLRISK_PARAMETERS_CACHE_TTL_SEC")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(60))
    })
}

fn cache_needs_refresh(cache: &ParametersCache, ttl: Duration) -> bool {
    match cache.last_fetch {
        None => true,
        Some(t) => t.elapsed() > ttl,
    }
}

pub async fn refresh_parameters_from_db(db: Option<&ParametersDb>) {
    let Some(db) = db else {
        return;
    };
    let ttl = parameters_cache_ttl();
    {
        let r = cache_store().read().ok();
        if let Some(c) = r {
            if !cache_needs_refresh(&c, ttl) {
                return;
            }
        }
    }

    let now = Instant::now();
    match db.fetch_parameters_map().await {
        Ok(map) => {
            if let Ok(mut w) = cache_store().write() {
                w.map = map;
                w.last_fetch = Some(now);
            }
        }
        Err(e) => {
            warn!(error = %e, "parameters table read failed; continuing with env vars only");
            if let Ok(mut w) = cache_store().write() {
                w.last_fetch = Some(now);
            }
        }
    }
}

// --- DB param_name keys (SOLRISK_ prefix for shared-DB safety) ---
// These are only relevant when DATABASE_URL is set.

const SOLRISK_X402_NETWORK: &str = "SOLRISK_X402_NETWORK";
const SOLRISK_X402_ACCEPTS_JSON: &str = "SOLRISK_X402_ACCEPTS_JSON";
const SOLRISK_X402_PAY_TO: &str = "SOLRISK_X402_PAY_TO";
const SOLRISK_MERCHANT_WALLET: &str = "SOLRISK_MERCHANT_WALLET";
const SOLRISK_X402_SCHEME: &str = "SOLRISK_X402_SCHEME";
const SOLRISK_X402_PAYMENT_TIMEOUT_SEC: &str = "SOLRISK_X402_PAYMENT_TIMEOUT_SEC";

/// Resolve a config value: DB row (if connected) → env var fallback.
/// `env_var` supports pipe-separated fallback names: "X402_PAY_TO|X402_PAY_TO_WALLET".
pub async fn resolve_string(
    db: Option<&ParametersDb>,
    param_key: &str,
    env_var: Option<&str>,
) -> Option<String> {
    if db.is_some() {
        refresh_parameters_from_db(db).await;
    }

    let from_db = cache_store()
        .read()
        .ok()
        .and_then(|c| c.map.get(param_key).cloned())
        .filter(|s| !s.is_empty());

    if from_db.is_some() {
        return from_db;
    }

    env_var
        .and_then(|name| {
            if name.contains('|') {
                for p in name.split('|') {
                    if let Ok(v) = std::env::var(p) {
                        if !v.trim().is_empty() {
                            return Some(v);
                        }
                    }
                }
                None
            } else {
                std::env::var(name).ok()
            }
        })
        .filter(|s| !s.is_empty())
}

// --- Resolver functions (plain env var names, same as spl-balance / aethervane) ---

pub async fn resolve_network(db: Option<&ParametersDb>) -> Option<String> {
    resolve_string(db, SOLRISK_X402_NETWORK, Some("X402_NETWORK")).await
}

pub async fn resolve_pay_to(db: Option<&ParametersDb>) -> Option<String> {
    resolve_string(
        db,
        SOLRISK_X402_PAY_TO,
        Some("X402_PAY_TO|X402_PAY_TO_WALLET"),
    )
    .await
}

pub async fn resolve_merchant_wallet(db: Option<&ParametersDb>) -> Option<String> {
    resolve_string(
        db,
        SOLRISK_MERCHANT_WALLET,
        Some("X402_MERCHANT_WALLET|MERCHANT_WALLET|SELLER_WALLET"),
    )
    .await
}

pub async fn resolve_scheme(db: Option<&ParametersDb>) -> Option<String> {
    resolve_string(db, SOLRISK_X402_SCHEME, Some("X402_SCHEME")).await
}

pub async fn resolve_accepts_json(db: Option<&ParametersDb>) -> Option<String> {
    resolve_string(
        db,
        SOLRISK_X402_ACCEPTS_JSON,
        Some("X402_ACCEPTS_JSON|X402_PAYMENT_ACCEPTS_JSON"),
    )
    .await
}

pub async fn resolve_timeout_sec(db: Option<&ParametersDb>, default: u64) -> u64 {
    let s = resolve_string(
        db,
        SOLRISK_X402_PAYMENT_TIMEOUT_SEC,
        Some("X402_PAYMENT_TIMEOUT_SECONDS"),
    )
    .await;
    if let Some(ref raw) = s {
        if let Ok(v) = raw.parse::<u64>() {
            return v;
        }
    }
    default
}
