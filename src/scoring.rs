//! Additive wallet risk scoring model (v1.3 — fund-flow + counterparty AML signals).

use crate::signals::chain::ChainSignals;
use crate::signals::labels::{allow_index, deny_index};
use serde::Serialize;

// 1.3.0: real fund-flow provenance (P1: `FUNDED_BY_LABELED` / `FUNDING_UNTRACED`) and
// recent-counterparty exposure (P1.2: `COUNTERPARTY_LABELED` + measured
// `unique_counterparties_30d`). 1.2.0: counterparty/program metrics null unless measured.
pub const SCORING_VERSION: &str = "1.3.0";

#[derive(Debug, Clone, Serialize)]
pub struct RiskResult {
    pub risk_score: u8,
    pub risk_band: &'static str,
    pub signals: ChainSignals,
    pub flags: Vec<String>,
    pub labels: Vec<LabelMatch>,
    pub confidence: f32,
    pub signal_quality: &'static str,
    pub scoring_version: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct LabelMatch {
    pub source: String,
    pub label: String,
    pub weight: i32,
}

/// Machine-readable action for agents: BLOCK on deny labels or CRITICAL band.
pub fn recommendation_from(band: &str, has_deny_label: bool, signal_quality: &str) -> &'static str {
    if has_deny_label || band == "CRITICAL" {
        "BLOCK"
    } else if band == "HIGH" || band == "MEDIUM" || signal_quality == "low" {
        "REVIEW"
    } else {
        "ALLOW"
    }
}

pub fn score_wallet(wallet: &str, signals: &ChainSignals) -> RiskResult {
    let mut score: i32 = 0;
    let mut flags: Vec<String> = Vec::new();
    let mut label_matches: Vec<LabelMatch> = Vec::new();

    if signals.is_fresh_funded {
        score += 25;
        flags.push("FRESH_FUNDED".to_string());
    }

    if signals.age_days > 30 && signals.tx_count_30d < 3 {
        score += 10;
        flags.push("DORMANT".to_string());
    }

    if signals.tx_count_30d > 50 && signals.sol_balance_lamports < 10_000_000 {
        score += 15;
        flags.push("DUST_PATTERN".to_string());
    }

    // P1 fund-flow: funded by a deny-labeled address (mixer / sanctioned / drainer) is a
    // strong risk signal an agent can't cheaply derive itself.
    if !signals.funder_labels.is_empty() {
        score += 40;
        flags.push("FUNDED_BY_LABELED".to_string());
    }
    // P1.1 multi-hop: a labeled address deeper in the funding chain (hop ≥ 2) — closer hops
    // weigh more (dilution with distance).
    if let Some(hop) = signals
        .funding_chain
        .iter()
        .filter(|h| h.hop >= 2 && !h.labels.is_empty())
        .min_by_key(|h| h.hop)
    {
        let weight = match hop.hop {
            2 => 25,
            _ => 15,
        };
        score += weight;
        flags.push(format!("FUNDING_CHAIN_LABELED:hop{}", hop.hop));
    }
    // Provenance we could not establish (deep history beyond our page budget, or a
    // genesis tx we couldn't map) — a mild caution, never a fabricated verdict.
    if matches!(
        signals.funding_source_risk.as_str(),
        "partial_history" | "genesis_untraceable"
    ) {
        score += 5;
        flags.push("FUNDING_UNTRACED".to_string());
    }

    // P1.2 fund-flow: recent transactions with deny-labeled counterparties (mixer /
    // sanctioned / drainer) — the "who does this wallet deal with" AML signal.
    if !signals.counterparty_labels.is_empty() {
        score += 30;
        flags.push(format!(
            "COUNTERPARTY_LABELED:{}",
            signals.counterparty_labels.len()
        ));
    }

    let deny_idx = deny_index();
    let deny_hits: Vec<_> = deny_idx.lookup(wallet).to_vec();
    for entry in &deny_hits {
        score += entry.weight;
        flags.push(format!("LABEL:{}", entry.label.to_uppercase()));
        label_matches.push(LabelMatch {
            source: entry.source.clone(),
            label: entry.label.clone(),
            weight: entry.weight,
        });
    }

    if signals.age_days > 0 {
        let age_bonus = ((signals.age_days as f64).ln() * 3.0).min(15.0) as i32;
        score -= age_bonus;
    }

    if !signals.counterparty_metrics_estimated
        && signals.tx_count_30d >= 10
        && signals.unique_counterparties_30d.unwrap_or(0) >= 5
    {
        score -= 10;
    }

    let allow_idx = allow_index();
    let allow_hits: Vec<_> = allow_idx.lookup(wallet).to_vec();
    for entry in &allow_hits {
        score += entry.weight;
        label_matches.push(LabelMatch {
            source: entry.source.clone(),
            label: entry.label.clone(),
            weight: entry.weight,
        });
    }

    let final_score = score.clamp(0, 100) as u8;
    let band = match final_score {
        0..=24 => "LOW",
        25..=49 => "MEDIUM",
        50..=74 => "HIGH",
        _ => "CRITICAL",
    };

    let signal_quality = compute_signal_quality(signals);
    let confidence = compute_confidence(signals, signal_quality);

    RiskResult {
        risk_score: final_score,
        risk_band: band,
        signals: signals.clone(),
        flags,
        labels: label_matches,
        confidence,
        signal_quality,
        scoring_version: SCORING_VERSION,
    }
}

fn compute_signal_quality(signals: &ChainSignals) -> &'static str {
    if signals.tx_count_total < 5 || signals.sig_pages_fetched < 2 {
        "low"
    } else if signals.counterparty_metrics_estimated {
        "medium"
    } else {
        "high"
    }
}

fn compute_confidence(signals: &ChainSignals, signal_quality: &str) -> f32 {
    let mut conf: f32 = match signal_quality {
        "high" => 0.65,
        "medium" => 0.55,
        _ => 0.45,
    };

    if signals.tx_count_total >= 50 {
        conf += 0.2;
    } else if signals.tx_count_total >= 10 {
        conf += 0.1;
    }

    if signals.age_days >= 90 {
        conf += 0.15;
    } else if signals.age_days >= 7 {
        conf += 0.05;
    }

    if signals.has_activity_48h {
        conf += 0.05;
    }

    if signals.spl_account_count >= 3 {
        conf += 0.05;
    }

    conf.min(0.95)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_signals() -> ChainSignals {
        ChainSignals {
            age_days: 200,
            tx_count_total: 100,
            tx_count_30d: 30,
            unique_counterparties_30d: Some(15),
            sol_balance_lamports: 500_000_000,
            spl_account_count: 5,
            has_activity_48h: true,
            program_diversity_30d: Some(8),
            is_fresh_funded: false,
            funding_source_risk: "traced_clean".to_string(),
            funder: None,
            funder_labels: vec![],
            funding_chain: vec![],
            counterparty_labels: vec![],
            first_seen_ts: 1700000000,
            latest_tx_ts: 1715000000,
            counterparty_metrics_estimated: true,
            sig_pages_fetched: 3,
        }
    }

    #[test]
    fn healthy_wallet_scores_low() {
        let result = score_wallet(
            "SomeHealthyWallet111111111111111111111111111",
            &base_signals(),
        );
        assert!(result.risk_score <= 24, "score={}", result.risk_score);
        assert_eq!(result.risk_band, "LOW");
        assert_eq!(result.scoring_version, SCORING_VERSION);
    }

    #[test]
    fn activity_bonus_skipped_when_counterparties_estimated() {
        let mut signals = base_signals();
        signals.is_fresh_funded = true;
        signals.funding_source_risk = "fresh_unknown".to_string();
        assert!(signals.counterparty_metrics_estimated);
        let wallet = "SomeHealthyWallet111111111111111111111111111";
        let estimated = score_wallet(wallet, &signals);
        let mut with_real = signals.clone();
        with_real.counterparty_metrics_estimated = false;
        let real = score_wallet(wallet, &with_real);
        assert!(
            real.risk_score < estimated.risk_score,
            "estimated={} real={}",
            estimated.risk_score,
            real.risk_score
        );
    }

    #[test]
    fn funded_by_labeled_address_raises_score() {
        let mut signals = base_signals();
        signals.funder = Some("Mixer1111111111111111111111111111111111111".to_string());
        signals.funder_labels = vec![crate::signals::tx::TxLabelHit {
            program: "Mixer1111111111111111111111111111111111111".to_string(),
            source: "test".to_string(),
            label: "mixer".to_string(),
        }];
        let clean = score_wallet(
            "SomeHealthyWallet111111111111111111111111111",
            &base_signals(),
        );
        let flagged = score_wallet("SomeHealthyWallet111111111111111111111111111", &signals);
        assert!(flagged.risk_score > clean.risk_score);
        assert!(flagged.flags.contains(&"FUNDED_BY_LABELED".to_string()));
    }

    #[test]
    fn labeled_second_hop_funder_raises_score_less_than_direct() {
        use crate::signals::funding::FundingHop;
        use crate::signals::tx::TxLabelHit;
        let hit = |addr: &str| {
            vec![TxLabelHit {
                program: addr.to_string(),
                source: "test".to_string(),
                label: "mixer".to_string(),
            }]
        };
        let wallet = "SomeHealthyWallet111111111111111111111111111";

        // Direct funder labeled → FUNDED_BY_LABELED (+40).
        let mut direct = base_signals();
        direct.funder_labels = hit("Mix1");
        direct.funding_chain = vec![FundingHop {
            address: "Mix1".into(),
            hop: 1,
            labels: hit("Mix1"),
        }];
        let direct_r = score_wallet(wallet, &direct);

        // Clean hop 1, labeled hop 2 → FUNDING_CHAIN_LABELED:hop2 (+25), no FUNDED_BY_LABELED.
        let mut second = base_signals();
        second.funding_chain = vec![
            FundingHop {
                address: "Clean1".into(),
                hop: 1,
                labels: vec![],
            },
            FundingHop {
                address: "Mix2".into(),
                hop: 2,
                labels: hit("Mix2"),
            },
        ];
        let second_r = score_wallet(wallet, &second);

        assert!(second_r.risk_score > score_wallet(wallet, &base_signals()).risk_score);
        assert!(second_r.risk_score < direct_r.risk_score);
        assert!(second_r
            .flags
            .iter()
            .any(|f| f == "FUNDING_CHAIN_LABELED:hop2"));
        assert!(!second_r.flags.contains(&"FUNDED_BY_LABELED".to_string()));
    }

    #[test]
    fn labeled_counterparty_raises_score() {
        let mut signals = base_signals();
        signals.counterparty_labels = vec![crate::signals::tx::TxLabelHit {
            program: "Drainer111111111111111111111111111111111111".to_string(),
            source: "test".to_string(),
            label: "drainer".to_string(),
        }];
        let clean = score_wallet(
            "SomeHealthyWallet111111111111111111111111111",
            &base_signals(),
        );
        let flagged = score_wallet("SomeHealthyWallet111111111111111111111111111", &signals);
        assert!(flagged.risk_score > clean.risk_score);
        assert!(flagged
            .flags
            .iter()
            .any(|f| f.starts_with("COUNTERPARTY_LABELED")));
    }

    #[test]
    fn fresh_funded_wallet_scores_medium() {
        let mut signals = base_signals();
        signals.is_fresh_funded = true;
        signals.age_days = 0;
        signals.tx_count_total = 2;
        signals.tx_count_30d = 2;
        signals.unique_counterparties_30d = Some(1);
        let result = score_wallet("FreshWallet1111111111111111111111111111111111", &signals);
        assert!(result.risk_score >= 25, "score={}", result.risk_score);
        assert!(result.flags.contains(&"FRESH_FUNDED".to_string()));
    }

    #[test]
    fn sanctioned_wallet_scores_critical() {
        let result = score_wallet(
            "WSSoJFMBEKBbAMwRqnMfjt1urtsFGBTMqjsqbBpVMpC",
            &base_signals(),
        );
        assert!(result.risk_score >= 75, "score={}", result.risk_score);
        assert_eq!(result.risk_band, "CRITICAL");
    }

    #[test]
    fn sanctioned_wallet_recommendation_block() {
        let result = score_wallet(
            "WSSoJFMBEKBbAMwRqnMfjt1urtsFGBTMqjsqbBpVMpC",
            &base_signals(),
        );
        let has_deny = result.labels.iter().any(|l| l.weight > 0);
        assert!(has_deny);
        assert_eq!(
            recommendation_from(result.risk_band, has_deny, result.signal_quality),
            "BLOCK"
        );
    }

    #[test]
    fn clean_wallet_recommendation_allow() {
        let result = score_wallet(
            "SomeHealthyWallet111111111111111111111111111",
            &base_signals(),
        );
        let has_deny = result.labels.iter().any(|l| l.weight > 0);
        assert_eq!(
            recommendation_from(result.risk_band, has_deny, result.signal_quality),
            "ALLOW"
        );
    }

    #[test]
    fn confidence_low_for_empty_wallet() {
        let signals = ChainSignals::default();
        let result = score_wallet("EmptyWallet11111111111111111111111111111111111", &signals);
        assert!(result.confidence <= 0.6);
        assert_eq!(result.signal_quality, "low");
    }
}
