//! Shared HTTP helpers for API handlers.

use crate::auth::dual::{DataAuth, DataAuthError};
use crate::constants::API_VERSION;
use crate::x402::models::SettlementProof;
use crate::x402::payment_handler::PaymentGateError;
use base64::Engine;
use serde_json::Value;
use vercel_runtime::{Body, Response};

pub const RISK_CORS_ALLOW_HEADERS: &str = "Content-Type, Authorization, PAYMENT-SIGNATURE, Payment-Required, PAYMENT-RESPONSE, X-API-Version, X-Correlation-ID";

pub fn cors_headers(builder: http::response::Builder) -> http::response::Builder {
    builder
        .header("Access-Control-Allow-Origin", "*")
        .header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
        .header("Access-Control-Allow-Headers", RISK_CORS_ALLOW_HEADERS)
        .header(
            "Access-Control-Expose-Headers",
            "Payment-Required, PAYMENT-RESPONSE, X-Correlation-ID, X-API-Version",
        )
}

pub fn json_response(status: u16, body: Value) -> Response<Body> {
    cors_headers(Response::builder().status(status))
        .header("Content-Type", "application/json")
        .header("X-API-Version", API_VERSION.to_string())
        .body(Body::Text(body.to_string()))
        .unwrap()
}

pub fn error_response(status: u16, code: &str, message: &str) -> Response<Body> {
    json_response(
        status,
        serde_json::json!({
            "error": code,
            "message": message,
            "code": code,
            "api_version": API_VERSION,
        }),
    )
}

/// Error response for failures that occur **after** payment settlement
/// (settle-before-work is deliberate on Solana — see `rpc_retry.rs`).
/// Attaches the settlement proof (`PAYMENT-RESPONSE` header + `settlement_sig`
/// body field) so the buyer can reconcile "paid, not served".
pub fn error_response_with_settlement(
    status: u16,
    code: &str,
    message: &str,
    auth: &DataAuth,
) -> Response<Body> {
    let mut body = serde_json::json!({
        "error": code,
        "message": message,
        "code": code,
        "api_version": API_VERSION,
    });
    if let Some(sig) = settlement_sig_from_auth(auth).filter(|s| !s.is_empty()) {
        body["settlement_sig"] = Value::String(sig);
    }

    let mut builder = cors_headers(Response::builder().status(status))
        .header("Content-Type", "application/json")
        .header("X-API-Version", API_VERSION.to_string());
    if let Some(proof) = settlement_from_auth(auth) {
        let hdr = proof.header_value();
        if !hdr.is_empty() {
            builder = builder.header("PAYMENT-RESPONSE", hdr);
        }
    }
    builder.body(Body::Text(body.to_string())).unwrap()
}

pub fn payment_402_response(
    payment_required: &crate::x402::models::PaymentRequired,
) -> Response<Body> {
    let payment_json = serde_json::to_string(payment_required).unwrap_or_else(|_| "{}".to_string());
    let payment_header = base64::engine::general_purpose::STANDARD.encode(payment_json.as_bytes());
    cors_headers(Response::builder().status(402))
        .header("Content-Type", "application/json")
        .header("Payment-Required", payment_header)
        .header("X-API-Version", API_VERSION.to_string())
        .body(Body::Text(payment_json))
        .unwrap()
}

pub fn subscribe_402_response(
    payment_required: &crate::x402::models::PaymentRequired,
) -> Response<Body> {
    let payment_json = serde_json::to_string(payment_required).unwrap_or_else(|_| "{}".to_string());
    cors_headers(Response::builder().status(402))
        .header("Content-Type", "application/json")
        .header("X-API-Version", API_VERSION.to_string())
        .body(Body::Text(payment_json))
        .unwrap()
}

pub fn ok_with_payment(body: Value, settlement: Option<&SettlementProof>) -> Response<Body> {
    let mut builder = cors_headers(Response::builder().status(200))
        .header("Content-Type", "application/json")
        .header("X-API-Version", API_VERSION.to_string());

    if let Some(proof) = settlement {
        let hdr = proof.header_value();
        if !hdr.is_empty() {
            builder = builder.header("PAYMENT-RESPONSE", hdr);
        }
    }

    builder.body(Body::Text(body.to_string())).unwrap()
}

pub fn handle_auth_error(err: DataAuthError) -> Response<Body> {
    match err {
        DataAuthError::Unauthorized { code, message } => error_response(401, code, &message),
        DataAuthError::RateLimited { code, message } => error_response(429, code, &message),
        DataAuthError::Payment(PaymentGateError::Required(pr)) => payment_402_response(&pr),
        DataAuthError::Payment(PaymentGateError::RequirementsUnavailable(msg)) => {
            error_response(503, "PAYMENT_REQUIREMENTS_UNAVAILABLE", &msg)
        }
    }
}

pub fn payer_from_auth(auth: &DataAuth) -> Option<String> {
    match auth {
        DataAuth::Subscription { payer, .. } => Some(payer.clone()),
        DataAuth::PerCall(proof) => proof.as_ref().and_then(|p| {
            p.response
                .get("payer")
                .or_else(|| p.response.get("buyer"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        }),
    }
}

pub fn settlement_sig_from_auth(auth: &DataAuth) -> Option<String> {
    match auth {
        DataAuth::Subscription { .. } => None,
        DataAuth::PerCall(proof) => proof.as_ref().and_then(|p| {
            p.response
                .get("transaction")
                .or_else(|| p.response.get("signature"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        }),
    }
}

pub fn settlement_from_auth(auth: &DataAuth) -> Option<&SettlementProof> {
    match auth {
        DataAuth::Subscription { .. } => None,
        DataAuth::PerCall(proof) => proof.as_ref(),
    }
}

pub fn extract_query_param<'a>(query: &'a str, key: &str) -> &'a str {
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

pub fn extract_tier_from_query(query: &str) -> Option<String> {
    let tier = extract_query_param(query, "tier");
    if tier.is_empty() {
        None
    } else {
        Some(tier.to_string())
    }
}
