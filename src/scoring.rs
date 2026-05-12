//! Additive risk scoring model.
//! Score = base(0) + penalties − bonuses, clamped to [0, 100].
//! All weights are transparent and versioned.

use crate::signals::chain::ChainSignals;
use crate::signals::labels::{allow_index, deny_index};
use serde::Serialize;

pub const SCORING_VERSION: &str = "1.0.0";

#[derive(Debug, Clone, Serialize)]
pub struct RiskResult {
    pub risk_score: u8,
    pub risk_band: &'static str,
    pub signals: ChainSignals,
    pub flags: Vec<String>,
    pub labels: Vec<LabelMatch>,
    pub confidence: f32,
    pub scoring_version: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct LabelMatch {
    pub source: String,
    pub label: String,
    pub weight: i32,
}

/// Compute the risk score from chain signals + label lookups.
pub fn score_wallet(wallet: &str, signals: &ChainSignals) -> RiskResult {
    let mut score: i32 = 0;
    let mut flags: Vec<String> = Vec::new();
    let mut label_matches: Vec<LabelMatch> = Vec::new();

    // --- Penalties (positive weight = increases risk) ---

    // Freshness penalty: funded <24h ago AND <5 tx
    if signals.is_fresh_funded {
        score += 25;
        flags.push("FRESH_FUNDED".to_string());
    }

    // Low activity penalty: very few tx in 30d for an old wallet
    if signals.age_days > 30 && signals.tx_count_30d < 3 {
        score += 10;
        flags.push("DORMANT".to_string());
    }

    // Dust farming: high tx count but very low SOL balance
    if signals.tx_count_30d > 50 && signals.sol_balance_lamports < 10_000_000 {
        // 50+ tx in 30d but <0.01 SOL
        score += 15;
        flags.push("DUST_PATTERN".to_string());
    }

    // Deny-list matches (OFAC, mixer, drainer, scam)
    let deny_hits = deny_index().lookup(wallet);
    for entry in deny_hits {
        score += entry.weight;
        flags.push(format!("LABEL:{}", entry.label.to_uppercase()));
        label_matches.push(LabelMatch {
            source: entry.source.clone(),
            label: entry.label.clone(),
            weight: entry.weight,
        });
    }

    // --- Bonuses (negative weight = decreases risk) ---

    // Age bonus: log-scaled, max -15
    if signals.age_days > 0 {
        let age_bonus = ((signals.age_days as f64).ln() * 3.0).min(15.0) as i32;
        score -= age_bonus;
    }

    // Activity bonus: healthy cadence with diversity
    if signals.tx_count_30d >= 10 && signals.unique_counterparties_30d >= 5 {
        score -= 10;
    }

    // Verified label bonus
    let allow_hits = allow_index().lookup(wallet);
    for entry in allow_hits {
        score += entry.weight; // weight is negative for allow entries
        label_matches.push(LabelMatch {
            source: entry.source.clone(),
            label: entry.label.clone(),
            weight: entry.weight,
        });
    }

    // --- Clamp and classify ---
    let final_score = score.clamp(0, 100) as u8;
    let band = match final_score {
        0..=24 => "LOW",
        25..=49 => "MEDIUM",
        50..=74 => "HIGH",
        _ => "CRITICAL",
    };

    // Confidence: reflects data coverage
    let confidence = compute_confidence(signals);

    RiskResult {
        risk_score: final_score,
        risk_band: band,
        signals: signals.clone(),
        flags,
        labels: label_matches,
        confidence,
        scoring_version: SCORING_VERSION,
    }
}

/// Confidence heuristic: how much data we had to work with.
/// Low tx count or very new wallet = lower confidence.
fn compute_confidence(signals: &ChainSignals) -> f32 {
    let mut conf: f32 = 0.5; // base

    // More tx history = more confidence
    if signals.tx_count_total >= 50 {
        conf += 0.2;
    } else if signals.tx_count_total >= 10 {
        conf += 0.1;
    }

    // Older wallet = more confidence
    if signals.age_days >= 90 {
        conf += 0.15;
    } else if signals.age_days >= 7 {
        conf += 0.05;
    }

    // Has recent activity = more confidence
    if signals.has_activity_48h {
        conf += 0.05;
    }

    // Has SPL accounts = richer signal
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
            unique_counterparties_30d: 15,
            sol_balance_lamports: 500_000_000,
            spl_account_count: 5,
            has_activity_48h: true,
            program_diversity_30d: 8,
            is_fresh_funded: false,
            funding_source_risk: "not_checked".to_string(),
            first_seen_ts: 1700000000,
            latest_tx_ts: 1715000000,
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
        assert!(result.confidence >= 0.8);
    }

    #[test]
    fn fresh_funded_wallet_scores_medium() {
        let mut signals = base_signals();
        signals.is_fresh_funded = true;
        signals.age_days = 0;
        signals.tx_count_total = 2;
        signals.tx_count_30d = 2;
        signals.unique_counterparties_30d = 1;
        let result = score_wallet("FreshWallet1111111111111111111111111111111111", &signals);
        assert!(result.risk_score >= 25, "score={}", result.risk_score);
        assert!(result.flags.contains(&"FRESH_FUNDED".to_string()));
    }

    #[test]
    fn sanctioned_wallet_scores_critical() {
        // Use a wallet from our denylist
        let result = score_wallet(
            "WSSoJFMBEKBbAMwRqnMfjt1urtsFGBTMqjsqbBpVMpC",
            &base_signals(),
        );
        assert!(result.risk_score >= 75, "score={}", result.risk_score);
        assert_eq!(result.risk_band, "CRITICAL");
    }

    #[test]
    fn confidence_low_for_empty_wallet() {
        let signals = ChainSignals::default();
        let result = score_wallet("EmptyWallet11111111111111111111111111111111111", &signals);
        assert!(result.confidence <= 0.6);
    }
}
