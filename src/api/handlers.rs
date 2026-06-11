//! Risk scoring API handlers (wallet, token, tx).

use crate::api::common::{
    error_response, extract_query_param, handle_auth_error, ok_with_payment, payer_from_auth,
    settlement_from_auth, settlement_sig_from_auth,
};
use crate::auth::dual::{authenticate_data_route, DataAuth};
use crate::constants::API_VERSION;
use crate::pricing::{self, ENDPOINT_TOKEN_RISK, ENDPOINT_TX_RISK, ENDPOINT_WALLET_RISK};
use crate::scoring;
use crate::scoring_token;
use crate::scoring_tx;
use crate::signals::chain;
use crate::signals::labels;
use crate::signals::token;
use crate::state::AppState;
use crate::x402::models::ResourceInfo;
use chrono::Utc;
use http::HeaderMap;
use std::sync::Arc;
use tracing::info;
use vercel_runtime::{Body, Response};

pub use crate::api::common::RISK_CORS_ALLOW_HEADERS;

pub async fn handle_health(state: Arc<AppState>) -> Response<Body> {
    let db_ok = if let Some(db) = state.db.as_ref() {
        db.fetch_parameters_map().await.is_ok()
    } else {
        false
    };
    let body = serde_json::json!({
        "status": "ok",
        "service": "solrisk",
        "version": env!("CARGO_PKG_VERSION"),
        "api_version": API_VERSION,
        "db_connected": db_ok,
        "wallet_scoring_version": scoring::SCORING_VERSION,
        "token_scoring_version": scoring_token::SCORING_VERSION,
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
) -> Option<serde_json::Value> {
    let db = state.db.as_deref()?;
    db.get_cached_score(endpoint, subject).await.ok().flatten()
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
    if let Some(db) = state.db.as_deref() {
        let _ = db
            .set_cached_score(endpoint, subject, score, band, body, scoring_version, 300)
            .await;
        let _ = db
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
            .await;
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

    if let Some(cached) = maybe_cached(&state, ENDPOINT_WALLET_RISK, wallet).await {
        return ok_with_payment(cached, None);
    }

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

    labels::refresh_labels_from_db(state.db.as_deref()).await;

    let signals = match chain::collect_chain_signals(&state.rpc_client, wallet).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(wallet = %wallet, error = %e, "chain signal collection failed");
            return error_response(
                503,
                "RPC_ERROR",
                &format!("Could not collect on-chain data for {wallet}: {e}"),
            );
        }
    };

    let result = scoring::score_wallet(wallet, &signals);
    let checked_at = Utc::now().to_rfc3339();
    let body = serde_json::json!({
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

    info!(wallet = %wallet, score = result.risk_score, band = result.risk_band, "wallet-risk scored");
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

    if let Some(cached) = maybe_cached(&state, ENDPOINT_TOKEN_RISK, mint).await {
        return ok_with_payment(cached, None);
    }

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

    let signals = match token::collect_token_signals(&state.rpc_client, mint).await {
        Ok(s) => s,
        Err(e) => {
            return error_response(
                503,
                "RPC_ERROR",
                &format!("Token signal collection failed: {e}"),
            );
        }
    };

    let result = scoring_token::score_token(mint, &signals);
    let body = serde_json::json!({
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
    headers: &HeaderMap,
    query: &str,
    state: Arc<AppState>,
) -> Response<Body> {
    let signature = extract_query_param(query, "signature");
    if signature.is_empty() || signature.len() < 80 {
        return error_response(
            400,
            "BAD_REQUEST",
            "Query parameter `signature` is required (base58 tx signature).",
        );
    }

    if let Some(cached) = maybe_cached(&state, ENDPOINT_TX_RISK, signature).await {
        return ok_with_payment(cached, None);
    }

    let resource = ResourceInfo {
        url: state
            .config
            .x402_resource_url_for_request(headers, "/api/v1/tx-risk", query),
        description: pricing::resource_description(ENDPOINT_TX_RISK).to_string(),
        mime_type: "application/json".to_string(),
    };

    let auth = match authenticate_data_route(&state, headers, ENDPOINT_TX_RISK, resource).await {
        Ok(a) => a,
        Err(e) => return handle_auth_error(e),
    };

    let result = scoring_tx::score_tx(signature);
    let body = serde_json::json!({
        "api_version": API_VERSION,
        "signature": signature,
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

    write_cache_and_log(
        &state,
        ENDPOINT_TX_RISK,
        signature,
        &body,
        result.risk_score as i32,
        result.risk_band,
        result.scoring_version,
        &auth,
        None,
    )
    .await;

    ok_with_payment(body, settlement_from_auth(&auth))
}
