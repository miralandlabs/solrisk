//! Transaction risk scoring (v1.0 stub — signature presence + age).

use serde::Serialize;

pub const SCORING_VERSION: &str = "1.0.0";

#[derive(Debug, Clone, Serialize)]
pub struct TxSignals {
    pub signature_len: usize,
    pub parsed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TxRiskResult {
    pub risk_score: u8,
    pub risk_band: &'static str,
    pub risk_domain: &'static str,
    pub signals: TxSignals,
    pub flags: Vec<String>,
    pub confidence: f32,
    pub signal_quality: &'static str,
    pub scoring_version: &'static str,
}

pub fn score_tx(signature: &str) -> TxRiskResult {
    let mut score: i32 = 0;
    let mut flags = Vec::new();

    if signature.len() < 80 || signature.len() > 100 {
        score += 40;
        flags.push("INVALID_SIGNATURE_FORMAT".to_string());
    }

    let final_score = score.clamp(0, 100) as u8;
    let band = match final_score {
        0..=24 => "LOW",
        25..=49 => "MEDIUM",
        50..=74 => "HIGH",
        _ => "CRITICAL",
    };

    TxRiskResult {
        risk_score: final_score,
        risk_band: band,
        risk_domain: "tx",
        signals: TxSignals {
            signature_len: signature.len(),
            parsed: false,
        },
        flags,
        confidence: 0.4,
        signal_quality: "low",
        scoring_version: SCORING_VERSION,
    }
}
