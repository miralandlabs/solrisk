//! Token rug-pull oriented risk scoring.

use crate::signals::token::TokenSignals;
use serde::Serialize;

pub const SCORING_VERSION: &str = "1.0.0";

#[derive(Debug, Clone, Serialize)]
pub struct TokenRiskResult {
    pub risk_score: u8,
    pub risk_band: &'static str,
    pub risk_domain: &'static str,
    pub signals: TokenSignals,
    pub flags: Vec<String>,
    pub confidence: f32,
    pub signal_quality: &'static str,
    pub scoring_version: &'static str,
}

pub fn score_token(mint: &str, signals: &TokenSignals) -> TokenRiskResult {
    let _ = mint;
    let mut score: i32 = 0;
    let mut flags = Vec::new();

    if !signals.mint_authority_revoked {
        score += 35;
        flags.push("MINT_AUTHORITY_ACTIVE".to_string());
    }
    if signals.freeze_authority_set {
        score += 20;
        flags.push("FREEZE_AUTHORITY_SET".to_string());
    }
    if signals.top10_holder_pct > 80.0 {
        score += 30;
        flags.push("TOP10_HOLDERS_GT_80PCT".to_string());
    }
    if signals.mint_age_days < 7 && signals.top10_holder_pct > 50.0 {
        score += 15;
        flags.push("YOUNG_MINT_HIGH_CONCENTRATION".to_string());
    }
    if !signals.liquidity_checked {
        flags.push("LOW_LIQUIDITY_UNKNOWN".to_string());
    }

    let final_score = score.clamp(0, 100) as u8;
    let band = match final_score {
        0..=24 => "LOW",
        25..=49 => "MEDIUM",
        50..=74 => "HIGH",
        _ => "CRITICAL",
    };

    let signal_quality = if signals.low_confidence {
        "low"
    } else if signals.liquidity_checked {
        "high"
    } else {
        "medium"
    };

    let mut confidence: f32 = if signals.low_confidence { 0.45 } else { 0.7 };
    if signals.supply_ui > 0.0 {
        confidence += 0.1;
    }
    if signals.mint_age_days > 30 {
        confidence += 0.05;
    }

    TokenRiskResult {
        risk_score: final_score,
        risk_band: band,
        risk_domain: "token",
        signals: signals.clone(),
        flags,
        confidence: confidence.min(0.9),
        signal_quality,
        scoring_version: SCORING_VERSION,
    }
}
