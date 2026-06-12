use {
    crate::{
        error::Error,
        parameters, pricing as endpoint_pricing,
        state::AppState,
        x402::{
            facilitator::{parse_payment_proof, FacilitatorClient},
            models::{PaymentRequired, ResourceInfo, SettlementProof},
            pricing,
        },
    },
    http::HeaderMap,
    serde_json::Value,
    tracing::{info, warn},
};

/// Outcome when the HTTP layer must distinguish **402** (payment / proof issue) from **503** (cannot build payment requirements).
#[derive(Debug)]
pub enum PaymentGateError {
    /// Standard x402 Payment Required body (includes `error` when proof invalid, etc.).
    Required(PaymentRequired),
    /// Paid mode is active but `accepts[]` / pricing could not be built (misconfiguration, DB, …).
    RequirementsUnavailable(String),
}

pub struct PaymentHandler;

/// Match a `/supported` kind entry to an `accepts[]` line.
///
/// pr402 may list **`exact`** / **`sla-escrow`** in discovery while sellers publish **`v2:solana:*`**
/// on HTTP 402. **`verifyBodyTemplate`** from **`build-*-payment-tx`** uses the wire form (`exact` /
/// `sla-escrow`); this helper treats wire and handler aliases as equivalent for gating.
fn supported_scheme_matches(kind_scheme: &str, line_scheme: &str) -> bool {
    kind_scheme == line_scheme
        || matches!(
            (kind_scheme, line_scheme),
            ("exact", "v2:solana:exact")
                | ("v2:solana:exact", "exact")
                | ("sla-escrow", "v2:solana:sla-escrow")
                | ("v2:solana:sla-escrow", "sla-escrow")
        )
}

fn onboard_scheme_entry<'a>(schemes: &'a Value, line_scheme: &str) -> Option<&'a Value> {
    schemes.get(line_scheme).or_else(|| match line_scheme {
        "exact" => schemes.get("v2:solana:exact"),
        "v2:solana:exact" => schemes.get("exact"),
        "sla-escrow" => schemes.get("v2:solana:sla-escrow"),
        "v2:solana:sla-escrow" => schemes.get("sla-escrow"),
        _ => None,
    })
}

fn is_exact_scheme(scheme: &str) -> bool {
    scheme == "exact" || scheme == "v2:solana:exact"
}

/// SLA-Escrow rail: pr402 verify matches `payTo` to the per-mint escrow PDA; `FundPayment.seller` is in `extra`.
fn is_sla_scheme(scheme: &str) -> bool {
    scheme == "sla-escrow" || scheme == "v2:solana:sla-escrow"
}

impl PaymentHandler {
    /// Free tier when no accepts JSON and `X402_PAYMENT_AMOUNT_USDC=0`.
    async fn payment_skipped(db: Option<&crate::db::ParametersDb>, endpoint: &str) -> bool {
        if parameters::resolve_accepts_json(db, endpoint)
            .await
            .is_some()
        {
            return false;
        }
        matches!(
            std::env::var("X402_PAYMENT_AMOUNT_USDC")
                .ok()
                .and_then(|s| s.parse::<f64>().ok()),
            Some(0.0)
        )
    }

    async fn build_requirement_lines(
        state: &AppState,
        endpoint: &str,
    ) -> Result<Vec<crate::x402::models::PaymentRequirementsLine>, Error> {
        let db = state.db.as_deref();
        parameters::refresh_parameters_from_db(db).await;

        let network = parameters::resolve_network(db, endpoint)
            .await
            .unwrap_or_else(|| state.config.x402_network.clone());

        let pay_to = parameters::resolve_pay_to(db, endpoint).await.ok_or_else(|| {
            Error::Internal(
                "X402_PAY_TO not set (exact: SplitVault; sla-escrow: overridden from facilitator discovery when X402_MERCHANT_WALLET is set)"
                    .into(),
            )
        })?;

        let scheme = parameters::resolve_scheme(db, endpoint)
            .await
            .unwrap_or_else(|| state.config.x402_scheme.clone());

        let timeout =
            parameters::resolve_timeout_sec(db, endpoint, state.config.x402_timeout_sec).await;

        if let Some(raw) = parameters::resolve_accepts_json(db, endpoint).await {
            let specs = pricing::parse_accepts_json(&raw)?;
            let lines =
                pricing::build_lines_from_specs(&network, &pay_to, &scheme, timeout, &specs)?;
            info!(
                endpoint = %endpoint,
                accept_count = lines.len(),
                scheme = %scheme,
                "x402 pricing from X402_ACCEPTS_JSON"
            );
            Ok(lines)
        } else if let Some(u) = state.config.x402_legacy_usdc_amount {
            let lines = pricing::legacy_usdc_line(&network, &pay_to, &scheme, u, timeout)?;
            info!(endpoint = %endpoint, legacy_usdc_ui = u, "x402 pricing from env amount");
            Ok(lines)
        } else {
            let default_usdc = endpoint_pricing::default_legacy_usdc(endpoint);
            let lines =
                pricing::legacy_usdc_line(&network, &pay_to, &scheme, default_usdc, timeout)?;
            info!(endpoint = %endpoint, default_usdc, "x402 pricing: endpoint default");
            Ok(lines)
        }
    }

    async fn enrich_accepts_with_fee_payer(
        facilitator: &FacilitatorClient,
        merchant_wallet: Option<String>,
        lines: Vec<crate::x402::models::PaymentRequirementsLine>,
    ) -> Result<Vec<Value>, Error> {
        let supported = facilitator.supported().await?;
        let capabilities_url = facilitator.capabilities_url();

        // Exact: onboard `vaultPda` is the SplitVault — use as `payTo`.
        // SLA-Escrow (this pr402): `payTo` must be the per-mint escrow PDA — resolve via `GET discovery`
        // with `asset=<accepts.asset>`; `FundPayment.seller` comes from extra.merchantWallet/beneficiary.
        let mut merchants_vaults = std::collections::HashMap::new();
        let need_onboard = merchant_wallet.is_some()
            && lines.iter().any(|row| is_exact_scheme(row.scheme.as_str()));
        if need_onboard {
            if let Some(seed) = merchant_wallet.as_ref() {
                let onboard = facilitator.onboard(seed).await.map_err(|e| {
                    Error::Internal(format!(
                        "facilitator onboard discovery failed for merchant wallet {}: {}",
                        seed, e
                    ))
                })?;
                merchants_vaults.insert(seed.clone(), onboard);
            }
        }

        let mut out = Vec::with_capacity(lines.len());
        let mut seen = std::collections::HashSet::new();

        for row in lines {
            // Deduplicate by scheme, network, and asset to ensure clean discovery
            let key = (row.scheme.clone(), row.network.clone(), row.asset.clone());
            if !seen.insert(key) {
                continue;
            }

            let mut line_val =
                serde_json::to_value(&row).map_err(|e| Error::Internal(e.to_string()))?;

            if let Some(kind) = supported.kinds.iter().find(|k| {
                k.network == row.network && supported_scheme_matches(&k.scheme, &row.scheme)
            }) {
                if let Some(extra) = &kind.extra {
                    // Start with the full extra metadata from Facilitator
                    let mut line_extra = extra.clone();

                    if let Some(obj) = line_extra.as_object_mut() {
                        obj.insert(
                            "capabilitiesUrl".to_string(),
                            serde_json::Value::String(capabilities_url.clone()),
                        );
                    }

                    // Exact: replace `payTo` with onboard `vaultPda` (SplitVault).
                    let mut vault_discovered = false;
                    if is_exact_scheme(row.scheme.as_str()) {
                        if let Some(seed) = merchant_wallet.as_ref() {
                            if let Some(onboard) = merchants_vaults.get(seed) {
                                if let Some(scheme_info) = onboard
                                    .get("schemes")
                                    .and_then(|s| onboard_scheme_entry(s, &row.scheme))
                                {
                                    if let Some(vault_pda) =
                                        scheme_info.get("vaultPda").and_then(|v| v.as_str())
                                    {
                                        info!(
                                            identity = seed,
                                            vault = vault_pda,
                                            scheme = row.scheme,
                                            "API-discovered SplitVault for exact rail payTo"
                                        );
                                        if let Some(line_obj) = line_val.as_object_mut() {
                                            line_obj.insert(
                                                "payTo".to_string(),
                                                serde_json::Value::String(vault_pda.to_string()),
                                            );
                                        }
                                        vault_discovered = true;
                                    }
                                }
                            }
                        }
                    }

                    // SLA-Escrow: replace `payTo` with `discovery(..., asset=mint).vaultPda` when merchant is set.
                    let mut sla_escrow_discovered = false;
                    if is_sla_scheme(row.scheme.as_str()) {
                        if let Some(seed) = merchant_wallet.as_ref() {
                            match facilitator
                                .discovery(seed, "sla-escrow", Some(row.asset.as_str()))
                                .await
                            {
                                Ok(info) => {
                                    if let Some(vault_pda) =
                                        info.get("vaultPda").and_then(|v| v.as_str())
                                    {
                                        info!(
                                            identity = seed,
                                            vault = vault_pda,
                                            scheme = row.scheme,
                                            asset = row.asset.as_str(),
                                            "facilitator discovery escrow PDA for sla-escrow payTo"
                                        );
                                        if let Some(line_obj) = line_val.as_object_mut() {
                                            line_obj.insert(
                                                "payTo".to_string(),
                                                serde_json::Value::String(vault_pda.to_string()),
                                            );
                                        }
                                        sla_escrow_discovered = true;
                                    }
                                }
                                Err(e) => {
                                    warn!(
                                        identity = seed,
                                        scheme = row.scheme,
                                        asset = row.asset.as_str(),
                                        error = %e,
                                        "SLA discovery failed; leaving configured payTo"
                                    );
                                }
                            }
                        }
                    }

                    // pr402: `merchantWallet` / `beneficiary` encode FundPayment.seller for SLA-Escrow.
                    if let Some(extra_obj) = line_extra.as_object_mut() {
                        if let Some(seed) = merchant_wallet.as_ref() {
                            extra_obj.insert(
                                "merchantWallet".to_string(),
                                serde_json::Value::String(seed.clone()),
                            );
                            if is_sla_scheme(row.scheme.as_str()) {
                                extra_obj.insert(
                                    "beneficiary".to_string(),
                                    serde_json::Value::String(seed.clone()),
                                );
                            }
                            if is_exact_scheme(row.scheme.as_str()) && !vault_discovered {
                                warn!(
                                    identity = seed,
                                    scheme = row.scheme,
                                    "merchant wallet set but no vaultPda from onboard; keeping configured payTo"
                                );
                            }
                            if is_sla_scheme(row.scheme.as_str()) && !sla_escrow_discovered {
                                warn!(
                                    identity = seed,
                                    scheme = row.scheme,
                                    "merchant wallet set but SLA discovery did not set payTo; verify may reject"
                                );
                            }
                        } else {
                            warn!(
                                scheme = row.scheme,
                                "merchant wallet not configured; leaving payTo as configured and omitting extra.merchantWallet (pr402 verify may fail for institutional rails)"
                            );
                        }
                    }

                    if let Some(line_obj) = line_val.as_object_mut() {
                        line_obj.insert("extra".to_string(), line_extra);
                    }
                }
            } else {
                warn!(
                    network = %row.network,
                    scheme = %row.scheme,
                    "no matching facilitator /supported kind for this accepts line; extra will omit institutional metadata"
                );
            }
            out.push(line_val);
        }
        Ok(out)
    }

    pub async fn get_payment_required(
        state: &AppState,
        endpoint: &str,
        resource: ResourceInfo,
        subscribe_hint: bool,
    ) -> Result<PaymentRequired, Error> {
        let db = state.db.as_deref();
        let merchant_wallet = parameters::resolve_merchant_wallet(db, endpoint)
            .await
            .or_else(|| state.config.x402_merchant_wallet.clone());

        let lines = Self::build_requirement_lines(state, endpoint).await?;
        let accepts =
            Self::enrich_accepts_with_fee_payer(&state.facilitator, merchant_wallet, lines).await?;

        let mut extensions = serde_json::json!({
            "pr402FacilitatorUrl": state.config.x402_facilitator_url,
        });
        if subscribe_hint {
            if let Some(obj) = extensions.as_object_mut() {
                obj.insert(
                    "subscribeUrl".to_string(),
                    serde_json::Value::String("/api/v1/subscribe".to_string()),
                );
                obj.insert(
                    "subscribeInfoUrl".to_string(),
                    serde_json::Value::String("/api/v1/subscribe/info".to_string()),
                );
            }
        }

        Ok(PaymentRequired {
            x402_version: 2,
            error: None,
            resource,
            accepts,
            extensions,
        })
    }

    /// x402 v2: `PAYMENT-SIGNATURE` only.
    pub fn extract_payment(headers: &HeaderMap) -> Option<String> {
        headers
            .get("payment-signature")
            .or_else(|| headers.get("PAYMENT-SIGNATURE"))
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    }

    pub async fn verify_and_settle(
        facilitator: &FacilitatorClient,
        body: &Value,
    ) -> Result<SettlementProof, Error> {
        facilitator.verify_and_settle(body).await
    }

    pub async fn check_payment(
        state: &AppState,
        endpoint: &str,
        headers: &HeaderMap,
        resource: ResourceInfo,
    ) -> Result<Option<SettlementProof>, PaymentGateError> {
        if Self::payment_skipped(state.db.as_deref(), endpoint).await {
            info!(endpoint = %endpoint, "payment skipped (legacy amount 0 and no accepts JSON)");
            return Ok(None);
        }

        let subscribe_hint = !endpoint.starts_with("/api/v1/subscribe");
        let payment_required = match Self::get_payment_required(
            state,
            endpoint,
            resource.clone(),
            subscribe_hint,
        )
        .await
        {
            Ok(p) => p,
            Err(e) => {
                warn!("payment config error (paid mode): {}", e);
                return Err(PaymentGateError::RequirementsUnavailable(format!(
                    "Payment is required for this service but pricing requirements could not be loaded: {}",
                    e
                )));
            }
        };

        match Self::extract_payment(headers) {
            None => Err(PaymentGateError::Required(payment_required.with_error(
                "PAYMENT-SIGNATURE header is required (x402 v2 payment proof)",
            ))),
            Some(raw) => {
                let proof = match parse_payment_proof(&raw) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!("invalid payment header: {}", e);
                        return Err(PaymentGateError::Required(
                            payment_required.with_error(format!("Invalid payment header: {}", e)),
                        ));
                    }
                };
                match Self::verify_and_settle(&state.facilitator, &proof.0).await {
                    Ok(settled) => Ok(Some(settled)),
                    Err(e) => {
                        warn!("verify/settle failed: {}", e);
                        Err(PaymentGateError::Required(payment_required.with_error(
                            format!("Payment verification or settlement failed: {}", e),
                        )))
                    }
                }
            }
        }
    }
}
