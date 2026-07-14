//! Fund-flow tracing (P1) — replaces the old `funding_source_from_sigs` heuristic (which
//! traced nothing) with a real, 1-hop funder trace.
//!
//! When signature pagination reached the wallet's **genesis** transaction, we fetch that
//! one tx, find who funded the wallet (the account that sent it the most SOL in its first
//! tx), and label-check that funder against the deny index. This is the core AML
//! provenance signal ("funded by a known mixer / sanctioned entity") an agent cannot
//! cheaply compute itself. Multi-hop tracing and recent-counterparty exposure are the
//! documented P1.1 / P1.2 slices.
//!
//! Honesty: if we did **not** reach genesis (deep history beyond our page budget) or the
//! genesis tx can't be mapped (address lookup tables), we say so — never guess a funder.

use crate::rpc_retry::{with_retry, RetryPolicy};
use crate::signals::labels::deny_index;
use crate::signals::tx::TxLabelHit;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_config::RpcTransactionConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_transaction_status::{EncodedTransaction, UiTransactionEncoding};
use std::str::FromStr;
use std::sync::Arc;

#[derive(Debug, Clone, serde::Serialize)]
pub struct FundingTrace {
    /// The original funder (base58), when the genesis tx was reached and mapped.
    pub funder: Option<String>,
    /// Deny-label hits on the funder (empty = clean or unknown).
    pub funder_labels: Vec<TxLabelHit>,
    /// Real classification: `no_history` | `partial_history` | `genesis_untraceable` |
    /// `labeled_bad` | `traced_clean`.
    pub classification: String,
    /// Whether pagination reached the wallet's first transaction.
    pub reached_genesis: bool,
}

impl FundingTrace {
    fn partial() -> Self {
        Self {
            funder: None,
            funder_labels: Vec::new(),
            classification: "partial_history".to_string(),
            reached_genesis: false,
        }
    }
    fn simple(classification: &str, reached_genesis: bool) -> Self {
        Self {
            funder: None,
            funder_labels: Vec::new(),
            classification: classification.to_string(),
            reached_genesis,
        }
    }
}

/// Trace the wallet's original funder from its genesis transaction. `reached_genesis`
/// must be true (pagination exhausted the history) for the trace to be attempted; the
/// caller knows this. `earliest_sig` is the oldest signature seen.
pub async fn trace_funder(
    rpc: &Arc<RpcClient>,
    wallet: &Pubkey,
    earliest_sig: Option<&str>,
    reached_genesis: bool,
) -> FundingTrace {
    if !reached_genesis {
        return FundingTrace::partial();
    }
    let Some(sig_str) = earliest_sig else {
        return FundingTrace::simple("no_history", true);
    };
    let Ok(sig) = Signature::from_str(sig_str) else {
        return FundingTrace::simple("genesis_untraceable", true);
    };

    let policy = RetryPolicy::from_env();
    let rpc_tx = Arc::clone(rpc);
    let fetched = with_retry(policy, "getTransaction", None, || {
        let rpc = Arc::clone(&rpc_tx);
        async move {
            let cfg = RpcTransactionConfig {
                encoding: Some(UiTransactionEncoding::Base64),
                commitment: Some(solana_commitment_config::CommitmentConfig::confirmed()),
                max_supported_transaction_version: Some(0),
            };
            rpc.get_transaction_with_config(&sig, cfg).await
        }
    })
    .await;

    let Ok(tx) = fetched else {
        // RPC unavailable — don't fabricate provenance.
        return FundingTrace::simple("genesis_untraceable", true);
    };

    let Some(funder) = extract_funder(&tx.transaction, wallet) else {
        return FundingTrace::simple("genesis_untraceable", true);
    };

    let funder_labels: Vec<TxLabelHit> = deny_index()
        .lookup(&funder)
        .iter()
        .map(|hit| TxLabelHit {
            program: funder.clone(),
            source: hit.source.clone(),
            label: hit.label.clone(),
        })
        .collect();

    let classification = if funder_labels.is_empty() {
        "traced_clean"
    } else {
        "labeled_bad"
    };
    FundingTrace {
        funder: Some(funder),
        funder_labels,
        classification: classification.to_string(),
        reached_genesis: true,
    }
}

/// From the genesis tx's pre/post balances, the funder is the non-wallet account that lost
/// the most SOL (i.e. sent it to the wallet). Returns `None` if the tx can't be mapped
/// (address lookup tables shift the balance/key alignment).
fn extract_funder(
    encoded: &solana_transaction_status::EncodedTransactionWithStatusMeta,
    wallet: &Pubkey,
) -> Option<String> {
    let meta = encoded.meta.as_ref()?;
    let EncodedTransaction::Binary(b64, _) = &encoded.transaction else {
        return None;
    };
    let tx = crate::signals::tx::decode_tx(b64).ok()?;
    let keys = tx.message.static_account_keys();

    // Balances index (static + loaded) must line up with static keys for a safe mapping.
    if meta.pre_balances.len() != keys.len() || meta.post_balances.len() != keys.len() {
        return None;
    }
    let wallet_idx = keys.iter().position(|k| k == wallet)?;
    // The wallet must have received in its genesis tx.
    if meta.post_balances[wallet_idx] <= meta.pre_balances[wallet_idx] {
        return None;
    }

    let mut best: Option<(usize, u64)> = None;
    for i in 0..keys.len() {
        if i == wallet_idx {
            continue;
        }
        let decrease = meta.pre_balances[i].saturating_sub(meta.post_balances[i]);
        if decrease > 0 && best.map(|(_, d)| decrease > d).unwrap_or(true) {
            best = Some((i, decrease));
        }
    }
    best.map(|(i, _)| keys[i].to_string())
}
