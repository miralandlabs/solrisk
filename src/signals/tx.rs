//! Pre-sign transaction screening (static instruction decode — no RPC).
//!
//! Decodes a base64 **unsigned** Solana transaction and flags the high-confidence,
//! deterministic danger patterns an agent must catch *before it signs*: interactions
//! with deny-labeled programs, SPL Token authority handoffs, token delegations, and
//! account closures. This is pure-CPU and always succeeds once the tx decodes, so the
//! money-critical verdict never depends on an RPC round-trip. Balance-delta drain
//! detection via `simulateTransaction` is the documented v1.1 slice.
//!
//! Honesty: what can't be statically resolved (programs behind address lookup tables)
//! is reported as reduced visibility, never silently treated as safe.

use crate::signals::labels::deny_index;
use base64::Engine;
use serde::Serialize;
use solana_sdk::transaction::VersionedTransaction;
use spl_token::instruction::TokenInstruction;

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
    /// v1 is static-only; balance-delta simulation is v1.1.
    pub simulated: bool,
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
