//! Risk scoring API handlers (wallet, token, tx).

use crate::api::common::{
    error_response, error_response_with_settlement, extract_query_param, handle_auth_error,
    ok_with_payment, payer_from_auth, settlement_from_auth, settlement_sig_from_auth,
};
use crate::auth::dual::{authenticate_data_route, DataAuth};
use crate::constants::API_VERSION;
use crate::pricing::{self, ENDPOINT_TOKEN_RISK, ENDPOINT_WALLET_RISK};
use crate::scoring;
use crate::scoring_token;
use crate::signals::chain;
use crate::signals::labels;
use crate::signals::token;
use crate::state::AppState;
use crate::x402::models::ResourceInfo;
use chrono::Utc;
use http::HeaderMap;
use std::{
    sync::{Arc, OnceLock, RwLock},
    time::{Duration, Instant},
};
use tracing::{info, warn};
use vercel_runtime::{Body, Response};

pub use crate::api::common::RISK_CORS_ALLOW_HEADERS;

const DB_HEALTH_TTL: Duration = Duration::from_secs(30);

struct DbHealthCache {
    ok: bool,
    checked_at: Instant,
}

static DB_HEALTH: OnceLock<RwLock<Option<DbHealthCache>>> = OnceLock::new();

async fn db_connected(state: &AppState) -> bool {
    let Some(db) = state.db.as_ref() else {
        return false;
    };
    if let Ok(guard) = DB_HEALTH.get_or_init(|| RwLock::new(None)).read() {
        if let Some(cache) = guard.as_ref() {
            if cache.checked_at.elapsed() < DB_HEALTH_TTL {
                return cache.ok;
            }
        }
    }
    let ok = db.ping().await.is_ok();
    if let Ok(mut guard) = DB_HEALTH.get_or_init(|| RwLock::new(None)).write() {
        *guard = Some(DbHealthCache {
            ok,
            checked_at: Instant::now(),
        });
    }
    ok
}

pub async fn handle_health(state: Arc<AppState>) -> Response<Body> {
    let db_ok = db_connected(&state).await;
    let coverage = labels::label_coverage();
    let body = serde_json::json!({
        "status": "ok",
        "service": "solrisk",
        "version": env!("CARGO_PKG_VERSION"),
        "api_version": API_VERSION,
        "db_connected": db_ok,
        "cluster": state.config.cluster_label(),
        "wallet_scoring_version": scoring::SCORING_VERSION,
        "token_scoring_version": scoring_token::SCORING_VERSION,
        "label_coverage": coverage,
    });
    crate::api::common::cors_headers(Response::builder().status(200))
        .header("Content-Type", "application/json")
        .header("X-API-Version", API_VERSION.to_string())
        .body(Body::Text(body.to_string()))
        .unwrap()
}

async fn maybe_cached(
    state: &AppState,
    endpoint: &str,
    subject: &str,
) -> Option<(serde_json::Value, chrono::DateTime<chrono::Utc>)> {
    let Some(db) = state.db.as_deref() else {
        info!(endpoint, subject, db_present = false, "state_db_absent");
        info!(endpoint, subject, db_present = false, "cache_read_skipped");
        return None;
    };
    let start = Instant::now();
    info!(endpoint, subject, db_present = true, "cache_read_start");
    match db.get_cached_score(endpoint, subject).await {
        Ok(Some(hit)) => {
            info!(
                endpoint,
                subject,
                elapsed_ms = start.elapsed().as_millis(),
                "cache_read_hit"
            );
            Some(hit)
        }
        Ok(None) => {
            info!(
                endpoint,
                subject,
                elapsed_ms = start.elapsed().as_millis(),
                "cache_read_miss"
            );
            None
        }
        Err(e) => {
            info!(
                endpoint,
                subject,
                elapsed_ms = start.elapsed().as_millis(),
                error = %e,
                "cache_read_error"
            );
            warn!(
                endpoint,
                subject,
                error = %e,
                "score cache read failed; scoring fresh"
            );
            None
        }
    }
}

fn enrich_cached(
    mut body: serde_json::Value,
    cached_at: chrono::DateTime<chrono::Utc>,
) -> serde_json::Value {
    if let Some(obj) = body.as_object_mut() {
        obj.insert("cache_hit".to_string(), serde_json::Value::Bool(true));
        obj.insert(
            "cached_at".to_string(),
            serde_json::Value::String(cached_at.to_rfc3339()),
        );
    }
    body
}

fn envelope_fields(
    cluster: &str,
    recommendation: &str,
    cache_hit: bool,
    cached_at: Option<chrono::DateTime<chrono::Utc>>,
) -> serde_json::Value {
    let mut v = serde_json::json!({
        "cluster": cluster,
        "recommendation": recommendation,
        "cache_hit": cache_hit,
    });
    if let Some(t) = cached_at {
        v["cached_at"] = serde_json::Value::String(t.to_rfc3339());
    }
    v
}

#[allow(clippy::too_many_arguments)]
async fn write_cache_and_log(
    state: &AppState,
    endpoint: &str,
    subject: &str,
    body: &serde_json::Value,
    score: i32,
    band: &str,
    scoring_version: &str,
    auth: &DataAuth,
    signals_json: Option<serde_json::Value>,
) {
    let Some(db) = state.db.as_deref() else {
        info!(endpoint, subject, db_present = false, "state_db_absent");
        info!(endpoint, subject, db_present = false, "cache_write_skipped");
        info!(
            endpoint,
            subject,
            db_present = false,
            "scoring_log_write_skipped"
        );
        return;
    };

    let cache_start = Instant::now();
    info!(endpoint, subject, score, band, "cache_write_start");
    match db
        .set_cached_score(endpoint, subject, score, band, body, scoring_version, 300)
        .await
    {
        Ok(_) => {
            info!(
                endpoint,
                subject,
                elapsed_ms = cache_start.elapsed().as_millis(),
                "cache_write_ok"
            );
        }
        Err(e) => {
            info!(
                endpoint,
                subject,
                elapsed_ms = cache_start.elapsed().as_millis(),
                error = %e,
                "cache_write_error"
            );
            warn!(
                endpoint,
                subject,
                error = %e,
                "score cache write failed"
            );
        }
    }

    let log_start = Instant::now();
    info!(endpoint, subject, score, band, "scoring_log_write_start");
    match db
        .log_scoring(
            endpoint,
            subject,
            score,
            band,
            scoring_version,
            signals_json,
            payer_from_auth(auth).as_deref(),
            None,
            settlement_sig_from_auth(auth).as_deref(),
        )
        .await
    {
        Ok(_) => {
            info!(
                endpoint,
                subject,
                elapsed_ms = log_start.elapsed().as_millis(),
                "scoring_log_write_ok"
            );
        }
        Err(e) => {
            info!(
                endpoint,
                subject,
                elapsed_ms = log_start.elapsed().as_millis(),
                error = %e,
                "scoring_log_write_error"
            );
        }
    }
}

pub async fn handle_wallet_risk(
    headers: &HeaderMap,
    query: &str,
    state: Arc<AppState>,
) -> Response<Body> {
    let wallet = extract_query_param(query, "wallet");
    if wallet.is_empty() || wallet.len() < 32 || wallet.len() > 44 {
        return error_response(
            400,
            "BAD_REQUEST",
            "Query parameter `wallet` is required (Solana base58 pubkey, 32–44 chars).",
        );
    }

    let cluster = state.config.cluster_label();
    let resource = ResourceInfo {
        url: state
            .config
            .x402_resource_url_for_request(headers, "/api/v1/wallet-risk", query),
        description: pricing::resource_description(ENDPOINT_WALLET_RISK).to_string(),
        mime_type: "application/json".to_string(),
    };

    let auth = match authenticate_data_route(&state, headers, ENDPOINT_WALLET_RISK, resource).await
    {
        Ok(a) => a,
        Err(e) => return handle_auth_error(e),
    };

    // Cache lookup only after auth: cached scores are part of the paid product
    // (disclosed via `cache_hit` / `cached_at`), not a free tier.
    if let Some((cached, cached_at)) = maybe_cached(&state, ENDPOINT_WALLET_RISK, wallet).await {
        return ok_with_payment(
            enrich_cached(cached, cached_at),
            settlement_from_auth(&auth),
        );
    }

    labels::refresh_labels_from_db(state.db.as_deref()).await;

    let signals = match chain::collect_chain_signals(&state.rpc_client, wallet).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(wallet = %wallet, error = %e, "chain signal collection failed");
            // Payment settles before RPC work (Solana blockhash expiry — see rpc_retry.rs),
            // so include the settlement proof for buyer-side reconciliation.
            return error_response_with_settlement(
                503,
                "RPC_ERROR",
                &format!("Could not collect on-chain data for {wallet}: {e}"),
                &auth,
            );
        }
    };

    let result = scoring::score_wallet(wallet, &signals);
    let has_deny = !result.labels.is_empty() && result.labels.iter().any(|l| l.weight > 0);
    let recommendation =
        scoring::recommendation_from(result.risk_band, has_deny, result.signal_quality);
    let checked_at = Utc::now().to_rfc3339();
    let mut body = serde_json::json!({
        "api_version": API_VERSION,
        "wallet": wallet,
        "risk_score": result.risk_score,
        "risk_band": result.risk_band,
        "checked_at": checked_at,
        "signals": result.signals,
        "flags": result.flags,
        "labels": result.labels,
        "confidence": result.confidence,
        "signal_quality": result.signal_quality,
        "scoring_version": result.scoring_version,
    });
    if let Some(obj) = body.as_object_mut() {
        for (k, v) in envelope_fields(cluster, recommendation, false, None)
            .as_object()
            .unwrap()
        {
            obj.insert(k.clone(), v.clone());
        }
    }

    write_cache_and_log(
        &state,
        ENDPOINT_WALLET_RISK,
        wallet,
        &body,
        result.risk_score as i32,
        result.risk_band,
        result.scoring_version,
        &auth,
        Some(serde_json::to_value(&result.signals).unwrap_or_default()),
    )
    .await;

    info!(wallet = %wallet, score = result.risk_score, band = result.risk_band, recommendation, "wallet-risk scored");
    ok_with_payment(body, settlement_from_auth(&auth))
}

pub async fn handle_token_risk(
    headers: &HeaderMap,
    query: &str,
    state: Arc<AppState>,
) -> Response<Body> {
    let mint = extract_query_param(query, "mint");
    if mint.is_empty() || mint.len() < 32 || mint.len() > 44 {
        return error_response(
            400,
            "BAD_REQUEST",
            "Query parameter `mint` is required (Solana base58 mint, 32–44 chars).",
        );
    }

    let cluster = state.config.cluster_label();
    let resource = ResourceInfo {
        url: state
            .config
            .x402_resource_url_for_request(headers, "/api/v1/token-risk", query),
        description: pricing::resource_description(ENDPOINT_TOKEN_RISK).to_string(),
        mime_type: "application/json".to_string(),
    };

    let auth = match authenticate_data_route(&state, headers, ENDPOINT_TOKEN_RISK, resource).await {
        Ok(a) => a,
        Err(e) => return handle_auth_error(e),
    };

    // Cache lookup only after auth (see wallet-risk handler).
    if let Some((cached, cached_at)) = maybe_cached(&state, ENDPOINT_TOKEN_RISK, mint).await {
        return ok_with_payment(
            enrich_cached(cached, cached_at),
            settlement_from_auth(&auth),
        );
    }

    let signals = match token::collect_token_signals(&state.rpc_client, mint).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(mint = %mint, error = %e, "token signal collection failed");
            return error_response_with_settlement(
                503,
                "RPC_ERROR",
                &format!("Token signal collection failed: {e}"),
                &auth,
            );
        }
    };

    let result = scoring_token::score_token(mint, &signals);
    let recommendation =
        scoring::recommendation_from(result.risk_band, false, result.signal_quality);
    let mut body = serde_json::json!({
        "api_version": API_VERSION,
        "mint": mint,
        "risk_domain": result.risk_domain,
        "risk_score": result.risk_score,
        "risk_band": result.risk_band,
        "checked_at": Utc::now().to_rfc3339(),
        "signals": result.signals,
        "flags": result.flags,
        "confidence": result.confidence,
        "signal_quality": result.signal_quality,
        "scoring_version": result.scoring_version,
    });
    if let Some(obj) = body.as_object_mut() {
        for (k, v) in envelope_fields(cluster, recommendation, false, None)
            .as_object()
            .unwrap()
        {
            obj.insert(k.clone(), v.clone());
        }
    }

    write_cache_and_log(
        &state,
        ENDPOINT_TOKEN_RISK,
        mint,
        &body,
        result.risk_score as i32,
        result.risk_band,
        result.scoring_version,
        &auth,
        Some(serde_json::to_value(&result.signals).unwrap_or_default()),
    )
    .await;

    ok_with_payment(body, settlement_from_auth(&auth))
}

pub async fn handle_tx_risk(
    _headers: &HeaderMap,
    query: &str,
    _state: Arc<AppState>,
) -> Response<Body> {
    let signature = extract_query_param(query, "signature");
    if signature.is_empty() || signature.len() < 80 {
        return error_response(
            400,
            "BAD_REQUEST",
            "Query parameter `signature` is required (base58 tx signature).",
        );
    }

    error_response(
        501,
        "NOT_IMPLEMENTED",
        "tx-risk is not available yet (planned v2.1 with getParsedTransaction). No payment required.",
    )
}
