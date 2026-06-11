//! Chain-derived signals from Solana RPC.

use crate::rpc_retry::{with_retry, RetryPolicy};
use chrono::Utc;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use std::sync::Arc;
use tracing::info;

#[derive(Debug, Clone, serde::Serialize)]
pub struct ChainSignals {
    pub age_days: u64,
    pub tx_count_total: u64,
    pub tx_count_30d: u64,
    pub unique_counterparties_30d: u64,
    pub sol_balance_lamports: u64,
    pub spl_account_count: u64,
    pub has_activity_48h: bool,
    pub program_diversity_30d: u64,
    pub is_fresh_funded: bool,
    pub funding_source_risk: String,
    pub first_seen_ts: i64,
    pub latest_tx_ts: i64,
    /// Counterparty/program counts are estimated without parsed transactions.
    pub counterparty_metrics_estimated: bool,
    pub sig_pages_fetched: u32,
}

impl Default for ChainSignals {
    fn default() -> Self {
        Self {
            age_days: 0,
            tx_count_total: 0,
            tx_count_30d: 0,
            unique_counterparties_30d: 0,
            sol_balance_lamports: 0,
            spl_account_count: 0,
            has_activity_48h: false,
            program_diversity_30d: 0,
            is_fresh_funded: false,
            funding_source_risk: "not_checked".to_string(),
            first_seen_ts: 0,
            latest_tx_ts: 0,
            counterparty_metrics_estimated: true,
            sig_pages_fetched: 0,
        }
    }
}

fn max_sig_pages() -> u32 {
    std::env::var("SOLRISK_MAX_SIG_PAGES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5)
}

fn funding_source_from_sigs(tx_count: u64, age_days: u64, first_seen_ts: i64) -> String {
    if tx_count == 0 {
        return "no_history".to_string();
    }
    if age_days < 1 && tx_count < 5 {
        return "fresh_unknown".to_string();
    }
    if first_seen_ts > 0 {
        "untraced".to_string()
    } else {
        "insufficient_data".to_string()
    }
}

pub async fn collect_chain_signals(
    rpc: &Arc<RpcClient>,
    wallet_str: &str,
) -> Result<ChainSignals, String> {
    let pubkey = Pubkey::from_str(wallet_str).map_err(|e| format!("invalid pubkey: {e}"))?;

    let policy = RetryPolicy::from_env();
    let now_ts = Utc::now().timestamp();
    let thirty_days_ago = now_ts - (30 * 86400);
    let forty_eight_hours_ago = now_ts - (48 * 3600);

    let rpc_bal = Arc::clone(rpc);
    let sol_balance = with_retry(policy, "getBalance", None, || {
        let rpc = Arc::clone(&rpc_bal);
        let pk = pubkey;
        async move { rpc.get_balance(&pk).await }
    })
    .await
    .unwrap_or(0);

    let rpc_tok = Arc::clone(rpc);
    let spl_account_count = with_retry(policy, "getTokenAccountsByOwner", None, || {
        let rpc = Arc::clone(&rpc_tok);
        let pk = pubkey;
        async move {
            let accounts = rpc
                .get_token_accounts_by_owner(
                    &pk,
                    solana_client::rpc_request::TokenAccountsFilter::ProgramId(spl_token::id()),
                )
                .await?;
            Ok(accounts.len() as u64)
        }
    })
    .await
    .unwrap_or(0);

    let mut all_sigs = Vec::new();
    let mut before: Option<solana_sdk::signature::Signature> = None;
    let max_pages = max_sig_pages();

    for page in 0..max_pages {
        let rpc_sig = Arc::clone(rpc);
        let pk = pubkey;
        let before_sig = before;

        let sigs_result = with_retry(policy, "getSignaturesForAddress", None, || {
            let rpc = Arc::clone(&rpc_sig);
            async move {
                let config = GetConfirmedSignaturesForAddress2Config {
                    before: before_sig,
                    limit: Some(100),
                    ..Default::default()
                };
                rpc.get_signatures_for_address_with_config(&pk, config)
                    .await
            }
        })
        .await;

        match sigs_result {
            Ok(sigs) => {
                let is_last = sigs.len() < 100;
                if let Some(last) = sigs.last() {
                    before = solana_sdk::signature::Signature::from_str(&last.signature).ok();
                }
                all_sigs.extend(sigs);
                if is_last {
                    break;
                }
            }
            Err(e) => {
                if page == 0 {
                    info!(wallet = wallet_str, error = %e, "sig fetch failed on first page");
                }
                break;
            }
        }
    }

    let tx_count_total = all_sigs.len() as u64;
    let mut first_seen_ts: i64 = 0;
    let mut latest_tx_ts: i64 = 0;
    let mut tx_count_30d: u64 = 0;
    let mut has_activity_48h = false;

    for sig in &all_sigs {
        if let Some(bt) = sig.block_time {
            if first_seen_ts == 0 || bt < first_seen_ts {
                first_seen_ts = bt;
            }
            if bt > latest_tx_ts {
                latest_tx_ts = bt;
            }
            if bt >= thirty_days_ago {
                tx_count_30d += 1;
            }
            if bt >= forty_eight_hours_ago {
                has_activity_48h = true;
            }
        }
    }

    let age_days = if first_seen_ts > 0 {
        ((now_ts - first_seen_ts) / 86400).max(0) as u64
    } else {
        0
    };

    let is_fresh_funded = age_days < 1 && tx_count_total < 5;
    let unique_counterparties_30d = (tx_count_30d as f64 * 0.6).round() as u64;
    let program_diversity_30d = (tx_count_30d as f64 * 0.3).round().min(20.0) as u64;
    let funding_source_risk = funding_source_from_sigs(tx_count_total, age_days, first_seen_ts);

    Ok(ChainSignals {
        age_days,
        tx_count_total,
        tx_count_30d,
        unique_counterparties_30d,
        sol_balance_lamports: sol_balance,
        spl_account_count,
        has_activity_48h,
        program_diversity_30d,
        is_fresh_funded,
        funding_source_risk,
        first_seen_ts,
        latest_tx_ts,
        counterparty_metrics_estimated: true,
        sig_pages_fetched: max_pages.min(tx_count_total.div_ceil(100) as u32),
    })
}
