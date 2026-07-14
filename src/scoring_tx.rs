//! Transaction risk verdict (v2.1 — static pre-sign screening + simulation enrichment).
//!
//! Consumes the decoded signals from [`crate::signals::tx`] and renders a
//! `SIGN / REVIEW / BLOCK` recommendation. BLOCK is reserved for direct control/asset
//! handoffs (authority change, known-bad program); REVIEW covers risky-but-sometimes-
//! legitimate actions (delegation, account close) and reduced visibility (lookup tables,
//! unlabeled programs). v2.1 adds best-effort simulation disclosure — `WOULD_FAIL` if the
//! tx would revert, and `OUTFLOW_VIA_UNKNOWN_PROGRAM` when a simulated SOL outflow leaves
//! the subject *through an opaque program* (a plain send is never escalated). Nothing here
//! fabricates a signal it did not observe.

use crate::signals::tx::TxSignals;
use serde::Serialize;

pub const SCORING_VERSION: &str = "2.1.0";

#[derive(Debug, Clone, Serialize)]
pub struct TxRiskResult {
    pub risk_score: u8,
    pub risk_band: &'static str,
    pub risk_domain: &'static str,
    /// Machine action for agents: `SIGN` / `REVIEW` / `BLOCK`.
    pub recommendation: &'static str,
    pub signals: TxSignals,
    pub flags: Vec<String>,
    pub confidence: f32,
    pub signal_quality: &'static str,
    pub scoring_version: &'static str,
}

pub fn score_tx(signals: &TxSignals) -> TxRiskResult {
    let mut score: i32 = 0;
    let mut flags: Vec<String> = Vec::new();

    // BLOCK-tier: direct control/asset handoff or a known-bad program.
    for hit in &signals.deny_program_hits {
        score += 60;
        flags.push(format!("DENY_PROGRAM:{}", hit.label.to_uppercase()));
    }
    for a in &signals.authority_handoffs {
        score += 60;
        flags.push(format!("AUTHORITY_HANDOFF:{a}"));
    }

    // REVIEW-tier: risky-but-sometimes-legitimate, or reduced visibility.
    if signals.delegations > 0 {
        score += 30;
        flags.push(format!("TOKEN_DELEGATION:{}", signals.delegations));
    }
    if signals.close_accounts > 0 {
        score += 20;
        flags.push(format!("CLOSE_ACCOUNT:{}", signals.close_accounts));
    }
    if signals.uses_lookup_tables {
        score += 15;
        flags.push("USES_LOOKUP_TABLES".to_string());
    }
    if !signals.unknown_programs.is_empty() {
        score += 15;
        flags.push(format!(
            "UNKNOWN_PROGRAMS:{}",
            signals.unknown_programs.len()
        ));
    }

    // v1.1 simulation enrichment (fields are None when sim didn't run — best-effort).
    if signals.simulation_error.is_some() {
        score += 10;
        flags.push("WOULD_FAIL".to_string());
    }
    // Sharpen the unknown-program concern only when simulation shows value actually
    // leaving the subject through an opaque program (avoids flagging legitimate sends).
    let outflow_via_unknown = signals
        .net_sol_change_lamports
        .map(|n| n < -crate::signals::tx::OUTFLOW_LAMPORTS_THRESHOLD)
        .unwrap_or(false)
        && !signals.unknown_programs.is_empty();
    if outflow_via_unknown {
        score += 25;
        if let Some(n) = signals.net_sol_change_lamports {
            flags.push(format!("OUTFLOW_VIA_UNKNOWN_PROGRAM:{}", -n));
        }
    }

    let final_score = score.clamp(0, 100) as u8;
    let band = match final_score {
        0..=24 => "LOW",
        25..=49 => "MEDIUM",
        50..=74 => "HIGH",
        _ => "CRITICAL",
    };

    let has_block = !signals.deny_program_hits.is_empty() || !signals.authority_handoffs.is_empty();
    let has_review = signals.delegations > 0
        || signals.close_accounts > 0
        || signals.uses_lookup_tables
        || !signals.unknown_programs.is_empty()
        || signals.simulation_error.is_some()
        || outflow_via_unknown;
    let recommendation = if has_block {
        "BLOCK"
    } else if has_review {
        "REVIEW"
    } else {
        "SIGN"
    };

    // Full static visibility unless programs hide behind a lookup table (or there were
    // no instructions to inspect).
    let signal_quality = if signals.instruction_count == 0 {
        "low"
    } else if signals.uses_lookup_tables {
        "medium"
    } else {
        "high"
    };
    let confidence = match signal_quality {
        "high" => 0.85,
        "medium" => 0.60,
        _ => 0.40,
    };

    TxRiskResult {
        risk_score: final_score,
        risk_band: band,
        risk_domain: "transaction",
        recommendation,
        signals: signals.clone(),
        flags,
        confidence,
        signal_quality,
        scoring_version: SCORING_VERSION,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signals::tx::{TxLabelHit, TxSignals};

    fn clean() -> TxSignals {
        TxSignals {
            fee_payer: Some("Payer1111111111111111111111111111111111111".into()),
            subject: Some("Payer1111111111111111111111111111111111111".into()),
            instruction_count: 2,
            programs: vec!["11111111111111111111111111111111".into()],
            uses_lookup_tables: false,
            simulated: false,
            simulation_error: None,
            net_sol_change_lamports: None,
            deny_program_hits: vec![],
            authority_handoffs: vec![],
            delegations: 0,
            close_accounts: 0,
            unknown_programs: vec![],
        }
    }

    #[test]
    fn clean_transfer_signs() {
        let r = score_tx(&clean());
        assert_eq!(r.recommendation, "SIGN");
        assert_eq!(r.risk_band, "LOW");
        assert_eq!(r.signal_quality, "high");
    }

    #[test]
    fn authority_handoff_blocks() {
        let mut s = clean();
        s.authority_handoffs.push("AccountOwner".into());
        let r = score_tx(&s);
        assert_eq!(r.recommendation, "BLOCK");
        assert!(r.risk_score >= 50);
        assert!(r.flags.iter().any(|f| f.contains("AUTHORITY_HANDOFF")));
    }

    #[test]
    fn deny_program_blocks() {
        let mut s = clean();
        s.deny_program_hits.push(TxLabelHit {
            program: "BadProg1111111111111111111111111111111111111".into(),
            source: "test".into(),
            label: "drainer".into(),
        });
        let r = score_tx(&s);
        assert_eq!(r.recommendation, "BLOCK");
        assert!(r.flags.iter().any(|f| f.contains("DENY_PROGRAM")));
    }

    #[test]
    fn delegation_reviews() {
        let mut s = clean();
        s.delegations = 1;
        let r = score_tx(&s);
        assert_eq!(r.recommendation, "REVIEW");
    }

    #[test]
    fn lookup_tables_reduce_visibility_and_review() {
        let mut s = clean();
        s.uses_lookup_tables = true;
        let r = score_tx(&s);
        assert_eq!(r.recommendation, "REVIEW");
        assert_eq!(r.signal_quality, "medium");
    }

    #[test]
    fn unknown_program_reviews() {
        let mut s = clean();
        s.unknown_programs
            .push("Unknown11111111111111111111111111111111111".into());
        let r = score_tx(&s);
        assert_eq!(r.recommendation, "REVIEW");
    }

    #[test]
    fn would_fail_reviews() {
        let mut s = clean();
        s.simulated = true;
        s.simulation_error = Some("InstructionError(0, Custom(1))".into());
        let r = score_tx(&s);
        assert_eq!(r.recommendation, "REVIEW");
        assert!(r.flags.iter().any(|f| f == "WOULD_FAIL"));
    }

    #[test]
    fn legit_outflow_alone_still_signs() {
        // A plain send (no unknown program) that moves SOL out must NOT be escalated —
        // the agent intended it. Only outflow *through an opaque program* is flagged.
        let mut s = clean();
        s.simulated = true;
        s.net_sol_change_lamports = Some(-5_000_000_000); // -5 SOL, but to known programs
        let r = score_tx(&s);
        assert_eq!(r.recommendation, "SIGN");
    }

    #[test]
    fn outflow_via_unknown_program_escalates() {
        let mut s = clean();
        s.simulated = true;
        s.unknown_programs
            .push("Opaque11111111111111111111111111111111111".into());
        s.net_sol_change_lamports = Some(-2_000_000_000); // -2 SOL through the opaque program
        let r = score_tx(&s);
        assert_eq!(r.recommendation, "REVIEW");
        assert!(r
            .flags
            .iter()
            .any(|f| f.starts_with("OUTFLOW_VIA_UNKNOWN_PROGRAM")));
    }
}
