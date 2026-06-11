//! DB-backed parameters with (service, endpoint, param_name) resolution + env fallback.

use crate::constants::SERVICE;
use crate::db::ParametersDb;
use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant};
use tracing::warn;

pub struct ParametersCache {
    /// Keys: `{endpoint}:{param_name}` and legacy `param_name` / `SOLRISK_*`.
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
    match db.fetch_service_parameters(SERVICE).await {
        Ok(rows) => {
            let mut map = HashMap::new();
            for (endpoint, param_name, param_value) in rows {
                map.insert(format!("{endpoint}:{param_name}"), param_value.clone());
                if endpoint == "*" {
                    map.entry(param_name.clone())
                        .or_insert_with(|| param_value.clone());
                }
                if param_name.starts_with("SOLRISK_") {
                    let stripped = param_name.strip_prefix("SOLRISK_").unwrap_or(&param_name);
                    map.entry(format!("*:{stripped}"))
                        .or_insert_with(|| param_value.clone());
                }
            }
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

fn lookup_cached(endpoint: &str, param_name: &str) -> Option<String> {
    let cache = cache_store().read().ok()?;
    let keys = [
        format!("{endpoint}:{param_name}"),
        format!("*:{param_name}"),
        format!("SOLRISK_{param_name}"),
        param_name.to_string(),
    ];
    for key in keys {
        if let Some(v) = cache.map.get(&key) {
            if !v.is_empty() {
                return Some(v.clone());
            }
        }
    }
    None
}

pub async fn resolve_string_for_endpoint(
    db: Option<&ParametersDb>,
    endpoint: &str,
    param_name: &str,
    env_var: Option<&str>,
) -> Option<String> {
    if db.is_some() {
        refresh_parameters_from_db(db).await;
    }

    if let Some(v) = lookup_cached(endpoint, param_name) {
        return Some(v);
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

pub async fn resolve_string(
    db: Option<&ParametersDb>,
    param_key: &str,
    env_var: Option<&str>,
) -> Option<String> {
    resolve_string_for_endpoint(db, "*", param_key, env_var).await
}

pub async fn resolve_network(db: Option<&ParametersDb>, endpoint: &str) -> Option<String> {
    resolve_string_for_endpoint(db, endpoint, "X402_NETWORK", Some("X402_NETWORK")).await
}

pub async fn resolve_pay_to(db: Option<&ParametersDb>, endpoint: &str) -> Option<String> {
    resolve_string_for_endpoint(
        db,
        endpoint,
        "X402_PAY_TO",
        Some("X402_PAY_TO|X402_PAY_TO_WALLET"),
    )
    .await
}

pub async fn resolve_merchant_wallet(db: Option<&ParametersDb>, endpoint: &str) -> Option<String> {
    resolve_string_for_endpoint(
        db,
        endpoint,
        "MERCHANT_WALLET",
        Some("X402_MERCHANT_WALLET|MERCHANT_WALLET|SELLER_WALLET"),
    )
    .await
}

pub async fn resolve_scheme(db: Option<&ParametersDb>, endpoint: &str) -> Option<String> {
    resolve_string_for_endpoint(db, endpoint, "X402_SCHEME", Some("X402_SCHEME")).await
}

pub async fn resolve_accepts_json(db: Option<&ParametersDb>, endpoint: &str) -> Option<String> {
    if let Some(v) = resolve_string_for_endpoint(
        db,
        endpoint,
        "X402_ACCEPTS_JSON",
        Some("X402_ACCEPTS_JSON|X402_PAYMENT_ACCEPTS_JSON"),
    )
    .await
    {
        return Some(v);
    }
    // Legacy flat key during deprecation window
    resolve_string(db, "SOLRISK_X402_ACCEPTS_JSON", None).await
}

pub async fn resolve_timeout_sec(db: Option<&ParametersDb>, endpoint: &str, default: u64) -> u64 {
    let s = resolve_string_for_endpoint(
        db,
        endpoint,
        "X402_PAYMENT_TIMEOUT_SEC",
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
