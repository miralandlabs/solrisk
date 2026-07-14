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
    /// `null` unless derived from parsed transactions
    /// (`counterparty_metrics_estimated == false`). Never synthesized.
    pub unique_counterparties_30d: Option<u64>,
    pub sol_balance_lamports: u64,
    pub spl_account_count: u64,
    pub has_activity_48h: bool,
    /// `null` unless derived from parsed transactions. Never synthesized.
    pub program_diversity_30d: Option<u64>,
    pub is_fresh_funded: bool,
    /// Real fund-flow classification (P1): `no_history` | `partial_history` |
    /// `genesis_untraceable` | `labeled_bad` | `traced_clean`.
    pub funding_source_risk: String,
    /// Original funder (base58) when the genesis tx was reached + mapped. Never guessed.
    pub funder: Option<String>,
    /// Deny-label hits on the funder (empty = clean or unknown).
    pub funder_labels: Vec<crate::signals::tx::TxLabelHit>,
    /// Deny-labeled recent counterparties (P1.2) — addresses this wallet transacted with
    /// that are on the deny list. Empty = clean or not measured.
    pub counterparty_labels: Vec<crate::signals::tx::TxLabelHit>,
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
            unique_counterparties_30d: None,
            sol_balance_lamports: 0,
            spl_account_count: 0,
            has_activity_48h: false,
            program_diversity_30d: None,
            is_fresh_funded: false,
            funding_source_risk: "not_checked".to_string(),
            funder: None,
            funder_labels: Vec::new(),
            counterparty_labels: Vec::new(),
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
    let mut pages_fetched: u32 = 0;
    // True once a page returns < 100 sigs — i.e. we've paginated back to the wallet's
    // first transaction. Required before we can honestly trace the original funder.
    let mut reached_genesis = false;

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
                pages_fetched += 1;
                let is_last = sigs.len() < 100;
                if let Some(last) = sigs.last() {
                    before = solana_sdk::signature::Signature::from_str(&last.signature).ok();
                }
                all_sigs.extend(sigs);
                if is_last {
                    reached_genesis = true;
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

    // P1 fund-flow trace: the wallet's oldest signature is the last one collected once we
    // reached genesis. Best-effort — never guesses a funder it didn't observe.
    let earliest_sig = if reached_genesis {
        all_sigs.last().map(|s| s.signature.as_str())
    } else {
        None
    };
    let trace =
        crate::signals::funding::trace_funder(rpc, &pubkey, earliest_sig, reached_genesis).await;

    // P1.2: parse a bounded window of recent (30d) txns for real counterparty / program
    // metrics + deny-labeled counterparty exposure. Best-effort — falls back to null.
    let recent_sigs: Vec<String> = all_sigs
        .iter()
        .filter(|s| {
            s.block_time
                .map(|bt| bt >= thirty_days_ago)
                .unwrap_or(false)
        })
        .map(|s| s.signature.clone())
        .collect();
    let activity = crate::signals::activity::analyze_recent_activity(
        rpc,
        &pubkey,
        &recent_sigs,
        crate::signals::activity::max_tx_parse(),
    )
    .await;

    let (unique_counterparties_30d, program_diversity_30d, counterparty_metrics_estimated) =
        if activity.measured {
            (
                Some(activity.unique_counterparties),
                Some(activity.program_diversity),
                false,
            )
        } else {
            (None, None, true)
        };

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
        funding_source_risk: trace.classification,
        funder: trace.funder,
        funder_labels: trace.funder_labels,
        counterparty_labels: activity.counterparty_labels,
        first_seen_ts,
        latest_tx_ts,
        counterparty_metrics_estimated,
        sig_pages_fetched: pages_fetched,
    })
}
