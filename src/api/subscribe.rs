//! Subscription purchase and info routes.

use crate::api::common::{
    error_response, extract_tier_from_query, ok_with_payment, subscribe_402_response,
};
use crate::pricing::{self, subscribe_endpoint_key, ALL_TIERS, PER_CALL_ENDPOINTS};
use crate::signals::labels;
use crate::state::AppState;
use crate::subscription::{
    self, is_valid_tier, tier_duration_secs, tier_label, JWT_PERSISTENCE_HINT,
};
use crate::x402::models::ResourceInfo;
use crate::x402::payment_handler::{PaymentGateError, PaymentHandler};
use chrono::Utc;
use http::HeaderMap;
use std::sync::Arc;
use tracing::info;
use vercel_runtime::{Body, Response};

/// `GET /api/v1/subscribe/info`
pub async fn handle_subscribe_info(state: Arc<AppState>) -> Response<Body> {
    let tiers: Vec<serde_json::Value> = ALL_TIERS
        .iter()
        .map(|t| {
            serde_json::json!({
                "tier": t,
                "label": tier_label(t),
                "durationSeconds": tier_duration_secs(t).unwrap_or(0),
                "endpoint": subscribe_endpoint_key(t),
            })
        })
        .collect();

    let data_routes: Vec<&str> = PER_CALL_ENDPOINTS
        .iter()
        .filter_map(|e| pricing::path_for_endpoint(e))
        .collect();

    labels::refresh_labels_from_db(state.db.as_deref()).await;
    let coverage = labels::label_coverage();

    crate::api::common::json_response(
        200,
        serde_json::json!({
            "service": "solrisk",
            "api_version": crate::constants::API_VERSION,
            "cluster": state.config.cluster_label(),
            "tiers": tiers,
            "dataRoutes": data_routes,
            "label_coverage": coverage,
            "persistenceHint": JWT_PERSISTENCE_HINT,
            "auth": "Authorization: Bearer <token> on data routes",
            "facilitatorUrl": state.config.x402_facilitator_url,
        }),
    )
}

/// `POST /api/v1/subscribe?tier=hourly|daily|monthly`
pub async fn handle_subscribe(
    headers: &HeaderMap,
    query: &str,
    state: Arc<AppState>,
) -> Response<Body> {
    let tier = match extract_tier_from_query(query) {
        Some(t) if is_valid_tier(&t) => t,
        _ => {
            return error_response(
                400,
                "BAD_REQUEST",
                "Query parameter `tier` is required (hourly, daily, or monthly).",
            );
        }
    };

    let endpoint_key = subscribe_endpoint_key(&tier);
    let path = "/api/v1/subscribe";
    let resource = ResourceInfo {
        url: state
            .config
            .x402_resource_url_for_request(headers, path, query),
        description: pricing::resource_description(&endpoint_key).to_string(),
        mime_type: "application/json".to_string(),
    };

    let settlement =
        match PaymentHandler::check_payment(&state, &endpoint_key, headers, resource).await {
            Ok(proof) => proof,
            Err(PaymentGateError::Required(pr)) => return subscribe_402_response(&pr),
            Err(PaymentGateError::RequirementsUnavailable(msg)) => {
                return error_response(503, "PAYMENT_REQUIREMENTS_UNAVAILABLE", &msg);
            }
        };

    let payer = settlement
        .as_ref()
        .and_then(|p| {
            p.response
                .get("payer")
                .or_else(|| p.response.get("buyer"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());

    let issued_at = Utc::now();
    let (token, expires_at) = match subscription::sign_subscription_token(&payer, &tier, issued_at)
    {
        Ok(v) => v,
        Err(e) => {
            return error_response(503, "JWT_ERROR", &e.to_string());
        }
    };

    if let Some(db) = state.db.as_deref() {
        let tx_sig = settlement.as_ref().and_then(|p| {
            p.response
                .get("transaction")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        });
        let _ = db
            .record_subscription(&payer, &tier, issued_at, expires_at, tx_sig.as_deref())
            .await;
    }

    info!(payer = %payer, tier = %tier, "subscription issued");

    let body = serde_json::json!({
        "success": true,
        "token": token,
        "tier": tier,
        "tierLabel": tier_label(&tier),
        "expiresAt": expires_at.to_rfc3339(),
        "durationSeconds": tier_duration_secs(&tier).unwrap_or(0),
        "usage": "Authorization: Bearer <token>",
        "persistenceHint": JWT_PERSISTENCE_HINT,
        "api_version": crate::constants::API_VERSION,
    });

    ok_with_payment(body, settlement.as_ref())
}
