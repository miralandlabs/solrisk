//! Fund-flow tracing (P1 / P1.1) — replaces the old `funding_source_from_sigs` heuristic
//! (which traced nothing) with a real, **multi-hop** funder trace.
//!
//! From the wallet's genesis tx we find its original funder (hop 1), then walk back up to
//! `SOLRISK_MAX_FUNDER_HOPS` hops (default 3), each time paginating that address to its own
//! genesis and extracting *its* funder. Every hop is deny-label-checked. This surfaces the
//! AML provenance an agent can't cheaply derive ("2 hops from a known mixer"). Walking
//! stops early at a labeled hop (bad source found) or when a hop's history is too deep to
//! reach genesis within the per-hop page budget (`SOLRISK_FUNDER_HOP_PAGES`, default 3).
//!
//! Honesty: partial history, or a genesis tx that can't be mapped (address lookup tables
//! shift key/balance alignment), is reported — never guessed.

use crate::rpc_retry::{with_retry, RetryPolicy};
use crate::signals::labels::deny_index;
use crate::signals::tx::TxLabelHit;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config;
use solana_client::rpc_config::RpcTransactionConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_transaction_status::{EncodedTransaction, UiTransactionEncoding};
use std::str::FromStr;
use std::sync::Arc;

/// One address in the funding chain. `hop` 1 = the wallet's direct funder.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FundingHop {
    pub address: String,
    pub hop: u32,
    /// Deny-label hits on this hop (empty = clean or unknown).
    pub labels: Vec<TxLabelHit>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FundingTrace {
    /// The direct funder (hop 1), when the genesis tx was reached and mapped.
    pub funder: Option<String>,
    /// Deny-label hits on the direct funder (empty = clean or unknown).
    pub funder_labels: Vec<TxLabelHit>,
    /// The traced chain, hop 1..N (N ≤ max hops; stops at a labeled or untraceable hop).
    pub funding_chain: Vec<FundingHop>,
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
            funding_chain: Vec::new(),
            classification: "partial_history".to_string(),
            reached_genesis: false,
        }
    }
    fn simple(classification: &str, reached_genesis: bool) -> Self {
        Self {
            funder: None,
            funder_labels: Vec::new(),
            funding_chain: Vec::new(),
            classification: classification.to_string(),
            reached_genesis,
        }
    }
}

fn max_funder_hops() -> u32 {
    std::env::var("SOLRISK_MAX_FUNDER_HOPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3)
        .max(1)
}

fn funder_hop_pages() -> u32 {
    std::env::var("SOLRISK_FUNDER_HOP_PAGES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3)
        .max(1)
}

fn deny_labels_for(address: &str) -> Vec<TxLabelHit> {
    deny_index()
        .lookup(address)
        .iter()
        .map(|hit| TxLabelHit {
            program: address.to_string(),
            source: hit.source.clone(),
            label: hit.label.clone(),
        })
        .collect()
}

/// Trace the wallet's funder chain from its genesis transaction. `reached_genesis` must be
/// true (the caller's pagination exhausted the wallet's history); `earliest_sig` is the
/// wallet's oldest signature.
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
    // Hop 1: the wallet's direct funder, from its genesis tx.
    let Some(hop1) = fetch_funder_from_sig(rpc, sig_str, wallet).await else {
        return FundingTrace::simple("genesis_untraceable", true);
    };

    let max_hops = max_funder_hops();
    let hop_pages = funder_hop_pages();
    let mut chain: Vec<FundingHop> = Vec::new();
    let mut current = hop1;
    let mut hop: u32 = 1;

    loop {
        let labels = deny_labels_for(&current);
        let labeled = !labels.is_empty();
        chain.push(FundingHop {
            address: current.clone(),
            hop,
            labels,
        });
        // Stop at a bad source, at the hop budget, or when we can't go deeper.
        if labeled || hop >= max_hops {
            break;
        }
        let Ok(cur_pk) = current.parse::<Pubkey>() else {
            break;
        };
        match funder_of(rpc, &cur_pk, hop_pages).await {
            Some(next) => {
                current = next;
                hop += 1;
            }
            None => break, // deeper history is untraceable within the per-hop budget
        }
    }

    let any_labeled = chain.iter().any(|h| !h.labels.is_empty());
    let classification = if any_labeled {
        "labeled_bad"
    } else {
        "traced_clean"
    };
    let (funder, funder_labels) = chain
        .first()
        .map(|h| (Some(h.address.clone()), h.labels.clone()))
        .unwrap_or((None, Vec::new()));

    FundingTrace {
        funder,
        funder_labels,
        funding_chain: chain,
        classification: classification.to_string(),
        reached_genesis: true,
    }
}

/// Fetch a genesis tx by signature and extract who funded `address` in it.
async fn fetch_funder_from_sig(
    rpc: &Arc<RpcClient>,
    sig_str: &str,
    address: &Pubkey,
) -> Option<String> {
    let sig = Signature::from_str(sig_str).ok()?;
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
    .await
    .ok()?;
    extract_funder(&fetched.transaction, address)
}

/// Find `address`'s own direct funder: paginate (bounded) to its genesis, then extract the
/// funder from that first tx. `None` when its history is too deep to reach genesis within
/// `hop_max_pages`, or the genesis can't be mapped — never a guess.
async fn funder_of(rpc: &Arc<RpcClient>, address: &Pubkey, hop_max_pages: u32) -> Option<String> {
    let policy = RetryPolicy::from_env();
    let mut before: Option<Signature> = None;
    let mut earliest: Option<String> = None;
    let mut reached = false;

    for _ in 0..hop_max_pages {
        let rpc_sig = Arc::clone(rpc);
        let before_sig = before;
        let addr = *address;
        let sigs = with_retry(policy, "getSignaturesForAddress", None, || {
            let rpc = Arc::clone(&rpc_sig);
            async move {
                let cfg = GetConfirmedSignaturesForAddress2Config {
                    before: before_sig,
                    limit: Some(100),
                    ..Default::default()
                };
                rpc.get_signatures_for_address_with_config(&addr, cfg).await
            }
        })
        .await
        .ok()?;

        let is_last = sigs.len() < 100;
        if let Some(last) = sigs.last() {
            before = Signature::from_str(&last.signature).ok();
            earliest = Some(last.signature.clone());
        }
        if is_last {
            reached = true;
            break;
        }
    }

    if !reached {
        return None;
    }
    let sig_str = earliest?;
    fetch_funder_from_sig(rpc, &sig_str, address).await
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
