//! Token mint signals for rug-pull risk scoring.

use crate::rpc_retry::{with_retry, RetryPolicy};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use std::sync::Arc;

#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct TokenSignals {
    pub mint_authority_revoked: bool,
    pub freeze_authority_set: bool,
    pub top10_holder_pct: f64,
    pub supply_ui: f64,
    pub decimals: u8,
    pub mint_age_days: u64,
    pub metadata_mutable: bool,
    pub low_confidence: bool,
    pub liquidity_checked: bool,
}

pub async fn collect_token_signals(
    rpc: &Arc<RpcClient>,
    mint_str: &str,
) -> Result<TokenSignals, String> {
    let mint = Pubkey::from_str(mint_str).map_err(|e| format!("invalid mint: {e}"))?;
    let policy = RetryPolicy::from_env();
    let mut signals = TokenSignals::default();

    let supply = with_retry(policy, "getTokenSupply", None, || {
        let rpc = Arc::clone(rpc);
        let m = mint;
        async move { rpc.get_token_supply(&m).await }
    })
    .await;

    if let Ok(s) = supply {
        signals.supply_ui = s.ui_amount.unwrap_or(0.0);
        signals.decimals = s.decimals;
    } else {
        signals.low_confidence = true;
    }

    let mint_info = with_retry(policy, "getAccountInfo", None, || {
        let rpc = Arc::clone(rpc);
        let m = mint;
        async move { rpc.get_account(&m).await }
    })
    .await;

    if let Ok(acc) = mint_info {
        if acc.data.len() >= 82 {
            let mint_auth_option = acc.data[0];
            let freeze_auth_option = acc.data[46];
            signals.mint_authority_revoked = mint_auth_option == 0;
            signals.freeze_authority_set = freeze_auth_option == 1;
        } else {
            signals.low_confidence = true;
        }
    } else {
        signals.low_confidence = true;
    }

    let largest = with_retry(policy, "getTokenLargestAccounts", None, || {
        let rpc = Arc::clone(rpc);
        let m = mint;
        async move { rpc.get_token_largest_accounts(&m).await }
    })
    .await;

    if let Ok(accounts) = largest {
        let total: f64 = accounts
            .iter()
            .take(10)
            .filter_map(|a| a.amount.ui_amount)
            .sum();
        if signals.supply_ui > 0.0 {
            signals.top10_holder_pct = (total / signals.supply_ui * 100.0).min(100.0);
        }
    } else {
        signals.low_confidence = true;
    }

    let sigs = with_retry(policy, "getSignaturesForAddress", None, || {
        let rpc = Arc::clone(rpc);
        let m = mint;
        async move {
            let config = GetConfirmedSignaturesForAddress2Config {
                limit: Some(1),
                ..Default::default()
            };
            rpc.get_signatures_for_address_with_config(&m, config).await
        }
    })
    .await;

    if let Ok(s) = sigs {
        if let Some(first) = s.last().and_then(|x| x.block_time) {
            let now = chrono::Utc::now().timestamp();
            signals.mint_age_days = ((now - first) / 86400).max(0) as u64;
        }
    }

    signals.liquidity_checked = false;
    Ok(signals)
}
