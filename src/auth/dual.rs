//! Dual auth: Bearer JWT (subscription) or per-call x402.

use http::HeaderMap;
use tracing::{error, warn};

use crate::db::ParametersDb;
use crate::state::AppState;
use crate::subscription::{self, SubscriptionClaims};
use crate::x402::models::ResourceInfo;
use crate::x402::models::SettlementProof;
use crate::x402::payment_handler::{PaymentGateError, PaymentHandler};

pub enum DataAuth {
    Subscription {
        payer: String,
        tier: String,
        claims: SubscriptionClaims,
    },
    PerCall(Option<SettlementProof>),
}

pub enum DataAuthError {
    Unauthorized { code: &'static str, message: String },
    RateLimited { code: &'static str, message: String },
    Payment(PaymentGateError),
}

fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .or_else(|| headers.get("Authorization"))
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn client_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .or_else(|| headers.get("X-Forwarded-For"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or(s).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

pub async fn check_global_rate_limit(
    db: Option<&ParametersDb>,
    headers: &HeaderMap,
) -> Result<(), DataAuthError> {
    let Some(db) = db else {
        return Ok(());
    };
    let ip = client_ip(headers);
    let key = format!("ip:{ip}");
    let limit = subscription::rate_limit_global_per_min();
    match db.check_and_increment_rate(&key, limit, 60).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(DataAuthError::RateLimited {
            code: "RATE_LIMIT_EXCEEDED",
            message: format!("Global rate limit exceeded ({limit}/min)"),
        }),
        Err(e) => {
            warn!(error = %e, "global rate limit check failed; allowing");
            Ok(())
        }
    }
}

async fn check_payer_rate_limit(
    db: Option<&ParametersDb>,
    payer: &str,
) -> Result<(), DataAuthError> {
    let Some(db) = db else {
        return Ok(());
    };
    let key = format!("payer:{payer}");
    let limit = subscription::rate_limit_per_payer_per_min();
    match db.check_and_increment_rate(&key, limit, 60).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(DataAuthError::RateLimited {
            code: "SUBSCRIBER_RATE_LIMIT_EXCEEDED",
            message: format!("Subscriber rate limit exceeded ({limit}/min)"),
        }),
        Err(e) => {
            warn!(error = %e, "payer rate limit check failed; allowing");
            Ok(())
        }
    }
}

async fn verify_bearer(state: &AppState, token: &str) -> Result<DataAuth, DataAuthError> {
    let claims = match subscription::verify_subscription_token(token) {
        Ok(c) => c,
        Err(e) => {
            if e.to_string().contains("ExpiredSignature") {
                return Err(DataAuthError::Unauthorized {
                    code: "TOKEN_EXPIRED",
                    message: "JWT expired — renew via POST /api/v1/subscribe".into(),
                });
            }
            return Err(DataAuthError::Unauthorized {
                code: "MISSING_TOKEN",
                message: format!("Invalid token: {e}"),
            });
        }
    };

    if claims.exp < chrono::Utc::now().timestamp() {
        return Err(DataAuthError::Unauthorized {
            code: "TOKEN_EXPIRED",
            message: "JWT expired — renew via POST /api/v1/subscribe".into(),
        });
    }

    if let Some(db) = state.db.as_deref() {
        match db.is_revoked(&claims.payer, claims.iat).await {
            Ok(true) => {
                return Err(DataAuthError::Unauthorized {
                    code: "TOKEN_REVOKED",
                    message: "Subscription revoked".into(),
                });
            }
            Ok(false) => {}
            // Fail CLOSED: a subscription we cannot confirm is un-revoked is treated as
            // invalid. Revocation must be authoritative — never serve a JWT we can't verify.
            Err(e) => {
                error!(error = %e, "revocation check failed; denying (fail-closed)");
                return Err(DataAuthError::Unauthorized {
                    code: "REVOCATION_UNVERIFIED",
                    message: "Could not verify subscription status; please retry".into(),
                });
            }
        }
    }

    check_payer_rate_limit(state.db.as_deref(), &claims.payer).await?;

    Ok(DataAuth::Subscription {
        payer: claims.payer.clone(),
        tier: claims.tier.clone(),
        claims,
    })
}

/// Authenticate a data route request: Bearer JWT first, then per-call x402.
pub async fn authenticate_data_route(
    state: &AppState,
    headers: &HeaderMap,
    endpoint: &str,
    resource: ResourceInfo,
) -> Result<DataAuth, DataAuthError> {
    check_global_rate_limit(state.db.as_deref(), headers).await?;

    if let Some(token) = extract_bearer(headers) {
        return verify_bearer(state, &token).await;
    }

    if PaymentHandler::extract_payment(headers).is_some() {
        match PaymentHandler::check_payment(state, endpoint, headers, resource).await {
            Ok(proof) => return Ok(DataAuth::PerCall(proof)),
            Err(e) => return Err(DataAuthError::Payment(e)),
        }
    }

    match PaymentHandler::get_payment_required(state, endpoint, resource, true).await {
        Ok(mut pr) => {
            pr.error = Some(
                "PAYMENT-SIGNATURE or Authorization: Bearer required (per-call x402 or subscription)"
                    .to_string(),
            );
            Err(DataAuthError::Payment(PaymentGateError::Required(pr)))
        }
        Err(e) => Err(DataAuthError::Payment(
            PaymentGateError::RequirementsUnavailable(format!(
                "Payment requirements unavailable: {e}"
            )),
        )),
    }
}
