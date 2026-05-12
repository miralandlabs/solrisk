//! Static label lookup from compiled-in JSONL files.
//! No DB required — labels are baked into the binary at compile time.

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Clone, Deserialize)]
pub struct LabelEntry {
    pub wallet: String,
    pub source: String,
    pub label: String,
    pub weight: i32,
    #[serde(default)]
    pub note: String,
}

/// All labels indexed by wallet pubkey for O(1) lookup.
pub struct LabelIndex {
    pub by_wallet: HashMap<String, Vec<LabelEntry>>,
}

impl LabelIndex {
    fn build(entries: Vec<LabelEntry>) -> Self {
        let mut by_wallet: HashMap<String, Vec<LabelEntry>> = HashMap::new();
        for entry in entries {
            by_wallet
                .entry(entry.wallet.clone())
                .or_default()
                .push(entry);
        }
        Self { by_wallet }
    }

    pub fn lookup(&self, wallet: &str) -> &[LabelEntry] {
        self.by_wallet
            .get(wallet)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Check if any wallet in the given set has a label with positive weight (deny).
    pub fn any_deny_match(&self, wallets: &[&str]) -> Vec<&LabelEntry> {
        let mut matches = Vec::new();
        for w in wallets {
            for entry in self.lookup(w) {
                if entry.weight > 0 {
                    matches.push(entry);
                }
            }
        }
        matches
    }
}

static DENY_INDEX: OnceLock<LabelIndex> = OnceLock::new();
static ALLOW_INDEX: OnceLock<LabelIndex> = OnceLock::new();

fn parse_jsonl(raw: &str) -> Vec<LabelEntry> {
    raw.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

pub fn deny_index() -> &'static LabelIndex {
    DENY_INDEX.get_or_init(|| {
        let raw = include_str!("../../data/denylist.jsonl");
        LabelIndex::build(parse_jsonl(raw))
    })
}

pub fn allow_index() -> &'static LabelIndex {
    ALLOW_INDEX.get_or_init(|| {
        let raw = include_str!("../../data/allowlist.jsonl");
        LabelIndex::build(parse_jsonl(raw))
    })
}
