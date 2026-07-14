//! Recent-counterparty & program-diversity metrics (P1.2).
//!
//! Parses a bounded window of the wallet's recent transactions to compute the
//! previously-`null` `unique_counterparties_30d` and `program_diversity_30d`, and — the
//! high-value part — label-checks each counterparty against the deny index ("this wallet
//! has been transacting with a known mixer / sanctioned / drainer address"). That is the
//! other half of the AML moat an agent cannot cheaply derive.
//!
//! Bounded + best-effort: it parses at most `cap` recent txns (env `SOLRISK_MAX_TX_PARSE`,
//! default 20) so latency/RPC stay bounded; when nothing was parsed the metrics stay
//! `estimated`/`null`, never synthesized. A tx that can't be mapped (address lookup tables
//! shift the key/balance alignment) is skipped, not guessed.

use crate::rpc_retry::{with_retry, RetryPolicy};
use crate::signals::labels::deny_index;
use crate::signals::tx::TxLabelHit;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_config::RpcTransactionConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_transaction_status::{EncodedTransaction, UiTransactionEncoding};
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct ActivityMetrics {
    pub unique_counterparties: u64,
    pub program_diversity: u64,
    /// Distinct counterparties that are on the deny list.
    pub counterparty_labels: Vec<TxLabelHit>,
    pub txns_parsed: u32,
    /// True once at least one tx was actually parsed (metrics are measured, not null).
    pub measured: bool,
}

impl ActivityMetrics {
    fn empty() -> Self {
        Self {
            unique_counterparties: 0,
            program_diversity: 0,
            counterparty_labels: Vec::new(),
            txns_parsed: 0,
            measured: false,
        }
    }
}

pub fn max_tx_parse() -> usize {
    std::env::var("SOLRISK_MAX_TX_PARSE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20)
}

/// Parse up to `cap` of the given (newest-first) recent signatures, aggregating unique
/// counterparties, program diversity, and deny-labeled counterparties.
pub async fn analyze_recent_activity(
    rpc: &Arc<RpcClient>,
    wallet: &Pubkey,
    recent_sigs: &[String],
    cap: usize,
) -> ActivityMetrics {
    if cap == 0 || recent_sigs.is_empty() {
        return ActivityMetrics::empty();
    }

    let deny = deny_index();
    let policy = RetryPolicy::from_env();
    let mut counterparties: HashSet<String> = HashSet::new();
    let mut programs: HashSet<String> = HashSet::new();
    let mut labeled: Vec<TxLabelHit> = Vec::new();
    let mut labeled_seen: HashSet<(String, String)> = HashSet::new();
    let mut parsed: u32 = 0;

    for sig_str in recent_sigs.iter().take(cap) {
        let Ok(sig) = Signature::from_str(sig_str) else {
            continue;
        };
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
            continue;
        };
        parsed += 1;

        if let Some((cps, progs)) = extract(&tx.transaction, wallet) {
            for p in progs {
                programs.insert(p);
            }
            for cp in cps {
                for hit in deny.lookup(&cp) {
                    if labeled_seen.insert((cp.clone(), hit.label.clone())) {
                        labeled.push(TxLabelHit {
                            program: cp.clone(),
                            source: hit.source.clone(),
                            label: hit.label.clone(),
                        });
                    }
                }
                counterparties.insert(cp);
            }
        }
    }

    ActivityMetrics {
        unique_counterparties: counterparties.len() as u64,
        program_diversity: programs.len() as u64,
        counterparty_labels: labeled,
        txns_parsed: parsed,
        measured: parsed > 0,
    }
}

/// From one tx: `(counterparties, programs)`. Counterparties are non-wallet, non-program
/// account keys whose SOL balance changed (value moved to/from the wallet's context).
/// Returns `None` when the tx can't be safely mapped (lookup-table key/balance misalignment).
fn extract(
    encoded: &solana_transaction_status::EncodedTransactionWithStatusMeta,
    wallet: &Pubkey,
) -> Option<(Vec<String>, Vec<String>)> {
    let meta = encoded.meta.as_ref()?;
    let EncodedTransaction::Binary(b64, _) = &encoded.transaction else {
        return None;
    };
    let tx = crate::signals::tx::decode_tx(b64).ok()?;
    let keys = tx.message.static_account_keys();
    if meta.pre_balances.len() != keys.len() || meta.post_balances.len() != keys.len() {
        return None;
    }
    let wallet_str = wallet.to_string();

    let mut programs: Vec<String> = Vec::new();
    for ix in tx.message.instructions() {
        if let Some(pk) = keys.get(ix.program_id_index as usize) {
            let s = pk.to_string();
            if !programs.contains(&s) {
                programs.push(s);
            }
        }
    }

    let mut counterparties: Vec<String> = Vec::new();
    for (i, key) in keys.iter().enumerate() {
        let s = key.to_string();
        if s == wallet_str || programs.contains(&s) {
            continue;
        }
        if meta.pre_balances[i] != meta.post_balances[i] && !counterparties.contains(&s) {
            counterparties.push(s);
        }
    }

    Some((counterparties, programs))
}
