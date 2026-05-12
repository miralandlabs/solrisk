//! Wallet risk scoring API handlers.

use crate::scoring;
use crate::signals::chain;
use crate::state::AppState;
use crate::x402::payment_handler::{PaymentGateError, PaymentHandler};
use base64::Engine;
use chrono::Utc;
use http::HeaderMap;
use std::sync::Arc;
use tracing::info;
use vercel_runtime::{Body, Response};

pub const RISK_CORS_ALLOW_HEADERS: &str =
    "Content-Type, Authorization, PAYMENT-SIGNATURE, Payment-Required, PAYMENT-RESPONSE, X-API-Version, X-Correlation-ID";

fn cors_headers(builder: http::response::Builder) -> http::response::Builder {
    builder
        .header("Access-Control-Allow-Origin", "*")
        .header("Access-Control-Allow-Methods", "GET, OPTIONS")
        .header("Access-Control-Allow-Headers", RISK_CORS_ALLOW_HEADERS)
        .header(
            "Access-Control-Expose-Headers",
            "Payment-Required, PAYMENT-RESPONSE, X-Correlation-ID, X-API-Version",
        )
}

fn json_response(status: u16, body: serde_json::Value) -> Response<Body> {
    cors_headers(Response::builder().status(status))
        .header("Content-Type", "application/json")
        .body(Body::Text(body.to_string()))
        .unwrap()
}

fn error_response(status: u16, code: &str, message: &str) -> Response<Body> {
    json_response(
        status,
        serde_json::json!({
            "error": code,
            "message": message,
            "code": code,
        }),
    )
}

/// `GET /health`
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
        "db_connected": db_ok,
        "scoring_version": scoring::SCORING_VERSION,
    });
    cors_headers(Response::builder().status(200))
        .header("Content-Type", "application/json")
        .body(Body::Text(body.to_string()))
        .unwrap()
}

/// `GET /api/v1/wallet-risk?wallet=<base58>`
pub async fn handle_wallet_risk(
    headers: &HeaderMap,
    query: &str,
    state: Arc<AppState>,
) -> Response<Body> {
    // Parse wallet from query string
    let wallet = extract_query_param(query, "wallet");
    if wallet.is_empty() || wallet.len() < 32 || wallet.len() > 44 {
        return error_response(
            400,
            "BAD_REQUEST",
            "Query parameter `wallet` is required (Solana base58 pubkey, 32–44 chars).",
        );
    }

    info!(wallet = %wallet, "handle_wallet_risk: start");

    // x402 payment gate
    let resource = crate::x402::models::ResourceInfo {
        url: state
            .config
            .x402_resource_url_for_request(headers, "/api/v1/wallet-risk", query),
        description: "Solana wallet risk score".to_string(),
        mime_type: "application/json".to_string(),
    };

    let settlement_proof = match PaymentHandler::check_payment(&state, headers, resource).await {
        Ok(proof) => proof,
        Err(PaymentGateError::Required(payment_required)) => {
            let payment_json =
                serde_json::to_string(&payment_required).unwrap_or_else(|_| "{}".to_string());
            let payment_header =
                base64::engine::general_purpose::STANDARD.encode(payment_json.as_bytes());
            return cors_headers(Response::builder().status(402))
                .header("Content-Type", "application/json")
                .header("Payment-Required", payment_header)
                .body(Body::Text(payment_json))
                .unwrap();
        }
        Err(PaymentGateError::RequirementsUnavailable(msg)) => {
            return error_response(503, "PAYMENT_REQUIREMENTS_UNAVAILABLE", &msg);
        }
    };

    // Collect chain signals
    let signals = match chain::collect_chain_signals(&state.rpc_client, wallet).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(wallet = %wallet, error = %e, "chain signal collection failed");
            return error_response(
                503,
                "RPC_ERROR",
                &format!("Could not collect on-chain data for {}: {}", wallet, e),
            );
        }
    };

    // Score
    let result = scoring::score_wallet(wallet, &signals);

    info!(
        wallet = %wallet,
        score = result.risk_score,
        band = result.risk_band,
        confidence = result.confidence,
        "handle_wallet_risk: scored"
    );

    // Build response
    let checked_at = Utc::now().to_rfc3339();
    let body = serde_json::json!({
        "wallet": wallet,
        "risk_score": result.risk_score,
        "risk_band": result.risk_band,
        "checked_at": checked_at,
        "signals": result.signals,
        "flags": result.flags,
        "labels": result.labels,
        "confidence": result.confidence,
        "scoring_version": result.scoring_version,
    });

    // Attach PAYMENT-RESPONSE header if settlement proof exists
    let mut builder = cors_headers(Response::builder().status(200))
        .header("Content-Type", "application/json")
        .header("X-API-Version", "1");

    if let Some(proof) = settlement_proof.as_ref() {
        let hdr = proof.header_value();
        if !hdr.is_empty() {
            builder = builder.header("PAYMENT-RESPONSE", hdr);
        }
    }

    builder.body(Body::Text(body.to_string())).unwrap()
}

fn extract_query_param<'a>(query: &'a str, key: &str) -> &'a str {
    query
        .split('&')
        .find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            if k == key {
                Some(v)
            } else {
                None
            }
        })
        .unwrap_or("")
}
