//! Build `accepts` lines from JSON specs + network presets.

use crate::error::Error;
use serde::Deserialize;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use super::models::PaymentRequirementsLine;

/// CAIP-2 Solana clusters we know mint presets for.
pub const SOLANA_MAINNET: &str = "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp";
pub const SOLANA_DEVNET: &str = "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1";

pub const NATIVE_SOL_MINT: &str = "11111111111111111111111111111111";
pub const WSOL_MINT: &str = "So11111111111111111111111111111111111111112";
pub const USDC_MAINNET: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
pub const USDC_DEVNET: &str = "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptSpec {
    /// `sol` | `wsol` | `usdc` | `spl`
    pub kind: String,
    #[serde(default)]
    pub mint: Option<String>,
    pub amount_ui: String,
    #[serde(default)]
    pub decimals: Option<u8>,
}

fn decimals_for_kind(kind: &str, spec: &AcceptSpec) -> Result<u8, Error> {
    if let Some(d) = spec.decimals {
        return Ok(d);
    }
    match kind {
        "sol" | "wsol" => Ok(9),
        "usdc" => Ok(6),
        "spl" => Err(Error::Internal(
            "\"spl\" accept kind requires \"decimals\" in SPL_BALANCE_X402_ACCEPTS_JSON".into(),
        )),
        _ => Err(Error::Internal(format!(
            "unknown accept kind \"{}\", expected sol|wsol|usdc|spl",
            kind
        ))),
    }
}

fn preset_mint_pubkey(kind: &str, network: &str, spec: &AcceptSpec) -> Result<Pubkey, Error> {
    let k = kind.to_ascii_lowercase();
    if k == "spl" {
        let m = spec
            .mint
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                Error::Internal("\"spl\" accept requires \"mint\" in accepts spec".into())
            })?;
        return Pubkey::from_str(m)
            .map_err(|_| Error::BadRequest(format!("invalid spl mint {}", m)));
    }

    match k.as_str() {
        "sol" => Pubkey::from_str(NATIVE_SOL_MINT).map_err(|e| Error::Internal(e.to_string())),
        "wsol" => Pubkey::from_str(WSOL_MINT).map_err(|e| Error::Internal(e.to_string())),
        "usdc" => {
            let s = if network == SOLANA_DEVNET
                || network.ends_with("EtWTRABZaYq6iMfeYKouRu166VU2xqa1")
            {
                USDC_DEVNET
            } else {
                USDC_MAINNET
            };
            Pubkey::from_str(s).map_err(|e| Error::Internal(e.to_string()))
        }
        _ => Err(Error::Internal(format!("unknown kind {}", kind))),
    }
}

fn ui_to_raw(amount_ui: &str, decimals: u8) -> Result<u64, Error> {
    let v: f64 = amount_ui
        .parse()
        .map_err(|_| Error::Internal(format!("invalid amountUi {}", amount_ui)))?;
    let scale = 10_f64.powi(decimals as i32);
    let raw = (v * scale).round();
    if raw < 0.0 || raw > u64::MAX as f64 {
        return Err(Error::Internal("amount out of range".into()));
    }
    Ok(raw as u64)
}

pub fn build_lines_from_specs(
    network: &str,
    pay_to: &str,
    scheme: &str,
    max_timeout_seconds: u64,
    specs: &[AcceptSpec],
) -> Result<Vec<PaymentRequirementsLine>, Error> {
    let mut out = Vec::with_capacity(specs.len());
    for spec in specs {
        let kind = spec.kind.to_ascii_lowercase();
        let dec = decimals_for_kind(&kind, spec)?;
        let mint = preset_mint_pubkey(&kind, network, spec)?;
        let raw = ui_to_raw(&spec.amount_ui, dec)?;
        if raw == 0 {
            continue;
        }
        out.push(PaymentRequirementsLine {
            scheme: scheme.to_string(),
            network: network.to_string(),
            amount: raw.to_string(),
            pay_to: pay_to.to_string(),
            max_timeout_seconds,
            asset: mint.to_string(),
            extra: None,
        });
    }
    if out.is_empty() {
        return Err(Error::Internal(
            "no non-zero payment accepts configured".into(),
        ));
    }
    Ok(out)
}

pub fn parse_accepts_json(json: &str) -> Result<Vec<AcceptSpec>, Error> {
    serde_json::from_str(json).map_err(|e| Error::Internal(format!("accepts JSON: {}", e)))
}

/// Single USDC line (legacy env `X402_PAYMENT_AMOUNT_USDC`).
pub fn legacy_usdc_line(
    network: &str,
    pay_to: &str,
    scheme: &str,
    amount_usdc_ui: f64,
    max_timeout_seconds: u64,
) -> Result<Vec<PaymentRequirementsLine>, Error> {
    let spec = AcceptSpec {
        kind: "usdc".to_string(),
        mint: None,
        amount_ui: format!("{}", amount_usdc_ui),
        decimals: None,
    };
    build_lines_from_specs(network, pay_to, scheme, max_timeout_seconds, &[spec])
}
