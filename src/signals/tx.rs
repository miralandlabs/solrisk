//! Pre-sign transaction screening (static instruction decode — no RPC).
//!
//! Decodes a base64 **unsigned** Solana transaction and flags the high-confidence,
//! deterministic danger patterns an agent must catch *before it signs*: interactions
//! with deny-labeled programs, SPL Token authority handoffs, token delegations, and
//! account closures. This is pure-CPU and always succeeds once the tx decodes, so the
//! money-critical verdict never depends on an RPC round-trip. Best-effort simulation
//! (`simulate`) enriches it with the subject's net SOL (v1.1) and SPL-token (v1.2) balance
//! changes; on any RPC failure the deterministic static verdict still stands.
//!
//! Honesty: what can't be statically resolved (programs behind address lookup tables)
//! is reported as reduced visibility, never silently treated as safe.

use crate::signals::labels::deny_index;
use base64::Engine;
use serde::Serialize;
use solana_account_decoder_client_types::{UiAccountData, UiAccountEncoding};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_config::{
    RpcSimulateTransactionAccountsConfig, RpcSimulateTransactionConfig,
};
use solana_client::rpc_request::TokenAccountsFilter;
use solana_commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::transaction::VersionedTransaction;
use spl_token::instruction::TokenInstruction;
use std::collections::HashSet;

/// Meaningful SOL outflow (0.01 SOL) — used to sharpen the unknown-program concern when
/// simulation shows value actually leaving the subject through an opaque program.
pub const OUTFLOW_LAMPORTS_THRESHOLD: i64 = 10_000_000;

/// Well-known programs that are expected in benign transactions.
const SYSTEM_PROGRAM: &str = "11111111111111111111111111111111";
const TOKEN_2022_PROGRAM: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
const ATA_PROGRAM: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
const COMPUTE_BUDGET_PROGRAM: &str = "ComputeBudget111111111111111111111111111111";
const MEMO_PROGRAM: &str = "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr";
const MEMO_PROGRAM_V1: &str = "Memo1UhkJRfHyvLMcVucJwxXeuD728EqVDDwQDxFMNo";

#[derive(Debug, Clone, Serialize)]
pub struct TxLabelHit {
    pub program: String,
    pub source: String,
    pub label: String,
}

/// A simulated net change in one token balance for the subject (v1.2).
#[derive(Debug, Clone, Serialize)]
pub struct TokenChange {
    pub mint: String,
    /// Raw base-unit delta (post − pre). Negative = tokens left the subject.
    pub delta_raw: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TxSignals {
    /// Fee payer (first static account key), whose safety we assess by default.
    pub fee_payer: Option<String>,
    /// Subject the verdict is about (`fee_payer` unless an `owner` override was given).
    pub subject: Option<String>,
    pub instruction_count: usize,
    /// Distinct program ids invoked (statically resolvable ones).
    pub programs: Vec<String>,
    /// v0 tx whose programs/accounts partly live in an address lookup table → we
    /// cannot fully vet them statically. Reduces confidence; never assumed safe.
    pub uses_lookup_tables: bool,
    /// True once best-effort `simulateTransaction` enrichment ran (v1.1).
    pub simulated: bool,
    /// On-chain simulation error — the tx would fail if signed. `None` = ran clean or not simulated.
    pub simulation_error: Option<String>,
    /// Subject's net SOL change from simulation, in lamports (negative = outflow). v1.1.
    pub net_sol_change_lamports: Option<i64>,
    /// Per-mint net token changes for the subject's touched token accounts (v1.2).
    /// Negative `delta_raw` = outflow. Empty when not simulated or no token accounts touched.
    pub net_token_changes: Vec<TokenChange>,
    // ---- findings ----
    /// Program on the deny list (known drainer/scam) — highest severity.
    pub deny_program_hits: Vec<TxLabelHit>,
    /// SPL Token `SetAuthority` handoffs (e.g. `AccountOwner`, `CloseAccount`).
    pub authority_handoffs: Vec<String>,
    /// SPL Token `Approve`/`ApproveChecked` delegations.
    pub delegations: usize,
    /// SPL Token `CloseAccount` occurrences.
    pub close_accounts: usize,
    /// Programs that are neither well-known nor labeled — can't be vouched for.
    pub unknown_programs: Vec<String>,
}

fn is_known_safe(program: &str) -> bool {
    program == SYSTEM_PROGRAM
        || program == spl_token::id().to_string()
        || program == TOKEN_2022_PROGRAM
        || program == ATA_PROGRAM
        || program == COMPUTE_BUDGET_PROGRAM
        || program == MEMO_PROGRAM
        || program == MEMO_PROGRAM_V1
}

/// Cheap pre-auth sanity: is the input plausibly base64 (std or url-safe)? Lets the
/// handler reject obvious garbage before charging, while keeping the SRM probe contract
/// (a format-valid sample reaches the 402 payment gate rather than a 400).
pub fn base64_ok(tx_b64: &str) -> bool {
    let t = tx_b64.trim();
    base64::engine::general_purpose::STANDARD.decode(t).is_ok()
        || base64::engine::general_purpose::URL_SAFE.decode(t).is_ok()
}

/// Decode a base64 (standard, then url-safe) serialized transaction.
pub fn decode_tx(tx_b64: &str) -> Result<VersionedTransaction, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(tx_b64.trim())
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(tx_b64.trim()))
        .map_err(|e| format!("transaction is not valid base64: {e}"))?;
    bincode::deserialize::<VersionedTransaction>(&bytes)
        .map_err(|e| format!("could not deserialize transaction: {e}"))
}

/// Statically analyze a decoded transaction. Infallible — produces signals; the verdict
/// is derived in `scoring_tx`.
pub fn analyze(tx: &VersionedTransaction, owner_override: Option<&str>) -> TxSignals {
    let keys = tx.message.static_account_keys();
    let fee_payer = keys.first().map(|k| k.to_string());
    let subject = owner_override
        .map(|s| s.to_string())
        .or_else(|| fee_payer.clone());

    let uses_lookup_tables = tx
        .message
        .address_table_lookups()
        .map(|l| !l.is_empty())
        .unwrap_or(false);

    let deny = deny_index();
    let mut signals = TxSignals {
        fee_payer,
        subject,
        instruction_count: tx.message.instructions().len(),
        programs: Vec::new(),
        uses_lookup_tables,
        simulated: false,
        simulation_error: None,
        net_sol_change_lamports: None,
        net_token_changes: Vec::new(),
        deny_program_hits: Vec::new(),
        authority_handoffs: Vec::new(),
        delegations: 0,
        close_accounts: 0,
        unknown_programs: Vec::new(),
    };

    for ix in tx.message.instructions() {
        let idx = ix.program_id_index as usize;
        // Program id beyond the static keys lives in an address lookup table — we can't
        // resolve or vet it here. Flag reduced visibility rather than assume it's safe.
        let Some(program_key) = keys.get(idx) else {
            signals.uses_lookup_tables = true;
            continue;
        };
        let program = program_key.to_string();
        if !signals.programs.contains(&program) {
            signals.programs.push(program.clone());
        }

        for hit in deny.lookup(&program) {
            signals.deny_program_hits.push(TxLabelHit {
                program: program.clone(),
                source: hit.source.clone(),
                label: hit.label.clone(),
            });
        }

        if program == spl_token::id().to_string() || program == TOKEN_2022_PROGRAM {
            if let Ok(token_ix) = TokenInstruction::unpack(&ix.data) {
                match token_ix {
                    TokenInstruction::SetAuthority { authority_type, .. } => {
                        signals
                            .authority_handoffs
                            .push(format!("{authority_type:?}"));
                    }
                    TokenInstruction::Approve { .. } | TokenInstruction::ApproveChecked { .. } => {
                        signals.delegations += 1;
                    }
                    TokenInstruction::CloseAccount => {
                        signals.close_accounts += 1;
                    }
                    _ => {}
                }
            }
        } else if !is_known_safe(&program) && !signals.unknown_programs.contains(&program) {
            // Unlabeled and not well-known: can't be vouched for → contributes to REVIEW.
            signals.unknown_programs.push(program);
        }
    }

    signals
}

/// Result of the best-effort simulation enrichment.
pub struct SimOutcome {
    pub simulated: bool,
    pub error: Option<String>,
    pub net_sol_change_lamports: Option<i64>,
    pub net_token_changes: Vec<TokenChange>,
}

impl SimOutcome {
    fn none() -> Self {
        Self {
            simulated: false,
            error: None,
            net_sol_change_lamports: None,
            net_token_changes: Vec::new(),
        }
    }
}

/// Parse an SPL token account's `(mint, amount)` from either encoding a node may return
/// (jsonParsed or base64). Returns `None` for anything that isn't a decodable token account.
fn parse_token_account(data: &UiAccountData) -> Option<(String, u64)> {
    match data {
        UiAccountData::Json(parsed) => {
            let info = parsed.parsed.get("info")?;
            let mint = info.get("mint")?.as_str()?.to_string();
            let amount = info
                .get("tokenAmount")?
                .get("amount")?
                .as_str()?
                .parse()
                .ok()?;
            Some((mint, amount))
        }
        UiAccountData::Binary(s, UiAccountEncoding::Base64) => {
            let bytes = base64::engine::general_purpose::STANDARD.decode(s).ok()?;
            if bytes.len() < 72 {
                return None; // not an initialized SPL token account
            }
            let mint = Pubkey::new_from_array(bytes[0..32].try_into().ok()?).to_string();
            let amount = u64::from_le_bytes(bytes[64..72].try_into().ok()?);
            Some((mint, amount))
        }
        _ => None,
    }
}

/// Best-effort v1.1/v1.2 enrichment: `simulateTransaction` (with `replaceRecentBlockhash`
/// so the unsigned tx simulates) to disclose whether the tx would fail, and the subject's
/// net **SOL** and **SPL-token** balance changes. Token accounts are bounded to those the
/// subject owns *and* the tx touches (only they can change). **Never fails the request** —
/// on any RPC error the caller keeps the deterministic static verdict. Token-2022 accounts
/// are a follow-up.
pub async fn simulate(
    rpc: &RpcClient,
    tx: &VersionedTransaction,
    subject: Option<&str>,
) -> SimOutcome {
    let Some(subject) = subject else {
        return SimOutcome::none();
    };
    let Ok(subject_pk) = subject.parse::<Pubkey>() else {
        return SimOutcome::none();
    };

    // Subject's SPL token accounts that this tx references (pubkey, mint, pre-amount).
    let tx_keys: HashSet<String> = tx
        .message
        .static_account_keys()
        .iter()
        .map(|k| k.to_string())
        .collect();
    let mut touched: Vec<(String, String, u64)> = Vec::new();
    if let Ok(accts) = rpc
        .get_token_accounts_by_owner(&subject_pk, TokenAccountsFilter::ProgramId(spl_token::id()))
        .await
    {
        for ka in accts {
            if tx_keys.contains(&ka.pubkey) {
                if let Some((mint, amount)) = parse_token_account(&ka.account.data) {
                    touched.push((ka.pubkey, mint, amount));
                }
            }
        }
    }

    let pre_sol = rpc.get_balance(&subject_pk).await.ok();

    let mut addresses = vec![subject.to_string()];
    addresses.extend(touched.iter().map(|(pk, _, _)| pk.clone()));
    let cfg = RpcSimulateTransactionConfig {
        sig_verify: false,
        replace_recent_blockhash: true,
        commitment: Some(CommitmentConfig::confirmed()),
        encoding: None,
        accounts: Some(RpcSimulateTransactionAccountsConfig {
            addresses,
            encoding: Some(UiAccountEncoding::Base64),
        }),
        inner_instructions: false,
        min_context_slot: None,
    };

    match rpc.simulate_transaction_with_config(tx, cfg).await {
        Ok(resp) => {
            let error = resp.value.err.map(|e| format!("{e:?}"));
            let accounts = resp.value.accounts.unwrap_or_default();
            // Sim returns accounts in the requested order: [subject, touched tokens...].
            let post_sol = accounts
                .first()
                .and_then(|o| o.as_ref())
                .map(|ui| ui.lamports);
            let net_sol_change_lamports = match (pre_sol, post_sol) {
                (Some(p), Some(q)) => Some(q as i64 - p as i64),
                _ => None,
            };

            let mut net_token_changes = Vec::new();
            for (i, (_, mint, pre)) in touched.iter().enumerate() {
                let post = accounts
                    .get(i + 1)
                    .and_then(|o| o.as_ref())
                    .and_then(|ui| parse_token_account(&ui.data))
                    .map(|(_, amt)| amt)
                    .unwrap_or(0); // account closed/emptied in the tx → 0
                let delta = post as i64 - *pre as i64;
                if delta != 0 {
                    net_token_changes.push(TokenChange {
                        mint: mint.clone(),
                        delta_raw: delta,
                    });
                }
            }

            SimOutcome {
                simulated: true,
                error,
                net_sol_change_lamports,
                net_token_changes,
            }
        }
        // RPC/sim unavailable → keep the static verdict; disclose nothing we didn't observe.
        Err(_) => SimOutcome::none(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::message::Message;
    use solana_sdk::signature::{Keypair, Signer};
    use solana_sdk::system_instruction;
    use solana_sdk::transaction::Transaction;

    fn b64(tx: &VersionedTransaction) -> String {
        base64::engine::general_purpose::STANDARD.encode(bincode::serialize(tx).unwrap())
    }

    /// A plain SOL transfer touches only the System program → no findings.
    #[test]
    fn plain_transfer_has_no_findings() {
        let payer = Keypair::new();
        let to = solana_sdk::pubkey::Pubkey::new_unique();
        let ix = system_instruction::transfer(&payer.pubkey(), &to, 1_000);
        let msg = Message::new(&[ix], Some(&payer.pubkey()));
        let vtx = VersionedTransaction::from(Transaction::new_unsigned(msg));

        let s = analyze(&vtx, None);
        assert_eq!(
            s.fee_payer.as_deref(),
            Some(payer.pubkey().to_string().as_str())
        );
        assert!(s.deny_program_hits.is_empty());
        assert!(s.authority_handoffs.is_empty());
        assert_eq!(s.delegations, 0);
        assert!(s.unknown_programs.is_empty());
        assert!(!s.uses_lookup_tables);
    }

    /// An SPL Token `Approve` (delegation) is detected.
    #[test]
    fn approve_is_flagged_as_delegation() {
        let owner = Keypair::new();
        let source = solana_sdk::pubkey::Pubkey::new_unique();
        let delegate = solana_sdk::pubkey::Pubkey::new_unique();
        let ix = spl_token::instruction::approve(
            &spl_token::id(),
            &source,
            &delegate,
            &owner.pubkey(),
            &[],
            5_000,
        )
        .unwrap();
        let msg = Message::new(&[ix], Some(&owner.pubkey()));
        let vtx = VersionedTransaction::from(Transaction::new_unsigned(msg));

        let s = analyze(&vtx, None);
        assert_eq!(s.delegations, 1);
    }

    #[test]
    fn round_trips_through_base64() {
        let payer = Keypair::new();
        let ix = system_instruction::transfer(
            &payer.pubkey(),
            &solana_sdk::pubkey::Pubkey::new_unique(),
            1,
        );
        let msg = Message::new(&[ix], Some(&payer.pubkey()));
        let vtx = VersionedTransaction::from(Transaction::new_unsigned(msg));
        let encoded = b64(&vtx);
        assert!(base64_ok(&encoded));
        let decoded = decode_tx(&encoded).unwrap();
        // payer + destination + System program.
        assert_eq!(decoded.message.static_account_keys().len(), 3);
    }
}
