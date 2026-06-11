//! Wallet label lookup: Postgres TTL cache + compile-time JSONL fallback.

use crate::db::ParametersDb;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant};
use tracing::warn;

#[derive(Debug, Clone, Deserialize)]
pub struct LabelEntry {
    pub wallet: String,
    pub source: String,
    pub label: String,
    pub weight: i32,
    #[serde(default)]
    pub note: String,
}

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
}

struct LabelCache {
    deny: LabelIndex,
    allow: LabelIndex,
    last_fetch: Option<Instant>,
}

static LABEL_CACHE: OnceLock<RwLock<LabelCache>> = OnceLock::new();

fn label_cache() -> &'static RwLock<LabelCache> {
    LABEL_CACHE.get_or_init(|| {
        RwLock::new(LabelCache {
            deny: static_deny_index(),
            allow: static_allow_index(),
            last_fetch: None,
        })
    })
}

fn labels_ttl() -> Duration {
    Duration::from_secs(
        std::env::var("SOLRISK_LABELS_CACHE_TTL_SEC")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(300),
    )
}

fn parse_jsonl(raw: &str) -> Vec<LabelEntry> {
    raw.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn static_deny_index() -> LabelIndex {
    let raw = include_str!("../../data/denylist.jsonl");
    LabelIndex::build(parse_jsonl(raw))
}

fn static_allow_index() -> LabelIndex {
    let raw = include_str!("../../data/allowlist.jsonl");
    LabelIndex::build(parse_jsonl(raw))
}

pub async fn refresh_labels_from_db(db: Option<&ParametersDb>) {
    let Some(db) = db else {
        return;
    };
    let ttl = labels_ttl();
    if let Ok(r) = label_cache().read() {
        if let Some(t) = r.last_fetch {
            if t.elapsed() < ttl {
                return;
            }
        }
    }

    match db.fetch_wallet_labels().await {
        Ok(rows) if !rows.is_empty() => {
            let mut deny = Vec::new();
            let mut allow = Vec::new();
            for entry in rows {
                if entry.weight > 0 {
                    deny.push(entry);
                } else {
                    allow.push(entry);
                }
            }
            if let Ok(mut w) = label_cache().write() {
                w.deny = LabelIndex::build(deny);
                w.allow = LabelIndex::build(allow);
                w.last_fetch = Some(Instant::now());
            }
        }
        Ok(_) => {}
        Err(e) => warn!(error = %e, "wallet labels DB read failed; using static JSONL"),
    }
}

pub fn deny_index() -> LabelIndex {
    label_cache()
        .read()
        .map(|c| LabelIndex {
            by_wallet: c.deny.by_wallet.clone(),
        })
        .unwrap_or_else(|_| static_deny_index())
}

pub fn allow_index() -> LabelIndex {
    label_cache()
        .read()
        .map(|c| LabelIndex {
            by_wallet: c.allow.by_wallet.clone(),
        })
        .unwrap_or_else(|_| static_allow_index())
}
