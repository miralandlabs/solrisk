//! Dynamic x402 resource manifest (SRM).

use crate::constants::API_VERSION;
use crate::pricing::{
    self, subscribe_endpoint_key, ALL_TIERS, ENDPOINT_TOKEN_RISK, ENDPOINT_TX_RISK,
    ENDPOINT_WALLET_RISK, PER_CALL_ENDPOINTS,
};
use crate::state::AppState;
use serde_json::json;

pub async fn build_x402_resources(state: &AppState, base_url: &str) -> serde_json::Value {
    let mut resources = Vec::new();

    for endpoint in PER_CALL_ENDPOINTS {
        let path = pricing::path_for_endpoint(endpoint).unwrap_or("");
        let sample = pricing::sample_query_for_endpoint(endpoint);
        resources.push(json!({
            "resourceType": "http",
            "url": format!("{base_url}{path}?{sample}"),
            "description": pricing::resource_description(endpoint),
            "mimeType": "application/json",
            "endpoint": endpoint,
        }));
    }

    for tier in ALL_TIERS {
        let key = subscribe_endpoint_key(tier);
        resources.push(json!({
            "resourceType": "http",
            "url": format!("{base_url}/api/v1/subscribe?tier={tier}"),
            "description": pricing::resource_description(&key),
            "mimeType": "application/json",
            "endpoint": key,
        }));
    }

    json!({
        "schemaVersion": "0.2.0",
        "apiVersion": API_VERSION,
        "service": "solrisk",
        "resources": resources,
        "dataEndpoints": [ENDPOINT_WALLET_RISK, ENDPOINT_TOKEN_RISK, ENDPOINT_TX_RISK],
        "facilitatorUrl": state.config.x402_facilitator_url,
    })
}
