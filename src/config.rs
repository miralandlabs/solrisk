use crate::error::Error;
use crate::x402::pricing::SOLANA_MAINNET;
use http::HeaderMap;
use std::env;

/// Application configuration
#[derive(Debug, Clone)]
pub struct Config {
    pub solana_rpc_url: String,
    /// Facilitator base including `/api/v1/facilitator` (pr402).
    pub x402_facilitator_url: String,
    pub x402_network: String,
    pub x402_pay_to: String,
    pub x402_merchant_wallet: Option<String>,
    pub x402_scheme: String,
    pub x402_timeout_sec: u64,
    /// Fallback USDC amount when `SOLRISK_X402_ACCEPTS_JSON` is unset (from `X402_PAYMENT_AMOUNT_USDC`).
    pub x402_legacy_usdc_amount: Option<f64>,
}

impl Config {
    pub fn from_env() -> Result<Self, Error> {
        let solana_rpc_url = env::var("RPC_URL")
            .unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".to_string());

        let x402_facilitator_url = env::var("X402_FACILITATOR_URL").map_err(|_| {
            Error::Internal(
                "X402_FACILITATOR_URL required (e.g. https://<host>/api/v1/facilitator for pr402)"
                    .into(),
            )
        })?;

        let x402_network = env::var("X402_NETWORK").unwrap_or_else(|_| SOLANA_MAINNET.to_string());

        let x402_pay_to = env::var("X402_PAY_TO")
            .or_else(|_| env::var("X402_PAY_TO_WALLET"))
            .map_err(|_| {
                Error::Internal("X402_PAY_TO not set (must be a SplitVault or Escrow PDA)".into())
            })?;

        let x402_merchant_wallet = env::var("X402_MERCHANT_WALLET")
            .or_else(|_| env::var("MERCHANT_WALLET"))
            .or_else(|_| env::var("SELLER_WALLET"))
            .ok();

        let x402_scheme = env::var("X402_SCHEME").unwrap_or_else(|_| "exact".to_string());

        let x402_timeout_sec = env::var("X402_PAYMENT_TIMEOUT_SECONDS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(300);

        let x402_legacy_usdc_amount = env::var("X402_PAYMENT_AMOUNT_USDC")
            .ok()
            .and_then(|s| s.parse().ok());

        Ok(Self {
            solana_rpc_url,
            x402_facilitator_url,
            x402_network,
            x402_pay_to,
            x402_merchant_wallet,
            x402_scheme,
            x402_timeout_sec,
            x402_legacy_usdc_amount,
        })
    }

    /// Default resource URL for 402 responses (override with `X402_RESOURCE_URL`).
    pub fn x402_resource_url(&self) -> String {
        env::var("X402_RESOURCE_URL")
            .unwrap_or_else(|_| "https://solrisk.signer-payer.me/api/v1/wallet-risk".to_string())
    }

    /// Resource URL matching this HTTP request (path + query + inferred absolute origin).
    pub fn x402_resource_url_for_request(
        &self,
        headers: &HeaderMap,
        path: &str,
        query: &str,
    ) -> String {
        let host = headers
            .get("x-forwarded-host")
            .or_else(|| headers.get("host"))
            .and_then(|h| h.to_str().ok())
            .filter(|h| !h.is_empty());
        let proto = headers
            .get("x-forwarded-proto")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("https");
        if let Some(host) = host {
            let path_query = if query.is_empty() {
                path.to_string()
            } else {
                format!("{}?{}", path, query)
            };
            format!("{}://{}{}", proto, host, path_query)
        } else {
            self.x402_resource_url()
        }
    }
}
