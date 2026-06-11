# solrisk wallet labels — ops playbook

## Overview

Labels drive the highest-value paid signal (deny/allow overrides). solrisk loads labels from:

1. **Postgres** `solrisk_wallet_labels` (preferred in production)
2. **Compile-time JSONL** [`data/denylist.jsonl`](../data/denylist.jsonl) and [`data/allowlist.jsonl`](../data/allowlist.jsonl) when DB is empty or unreachable

Cache TTL: `SOLRISK_LABELS_CACHE_TTL_SEC` (default **300** seconds).

## Weight convention

| Weight | Meaning |
|--------|---------|
| `> 0` | Deny — increases risk score |
| `< 0` | Allow — decreases risk score |
| `100` | OFAC / sanctions (forces `CRITICAL` + `recommendation: BLOCK`) |
| `70–90` | Confirmed drainer / scam operator |
| `-5` to `-15` | Verified protocol or asset |

## Regenerate seed files

```bash
python3 scripts/build_label_seeds.py
```

This refreshes:

- `data/denylist.jsonl` (OFAC + community blacklist)
- `data/allowlist.jsonl` (verified protocols)
- `migrations/labels-seed.sql`

Then apply to Postgres:

```bash
psql "$DATABASE_URL" -f migrations/labels-seed.sql
```

Redeploy Vercel so the Rust binary picks up new JSONL (rebuild required for compile-time fallback).

## Manual add (production hotfix)

```sql
INSERT INTO solrisk_wallet_labels (wallet_pubkey, source, label, weight, evidence_url)
VALUES ('<base58>', 'internal', 'phishing_drainer', 85, 'https://...')
ON CONFLICT (wallet_pubkey, source, label) DO UPDATE SET
  weight = EXCLUDED.weight,
  evidence_url = EXCLUDED.evidence_url;
```

Wait up to `SOLRISK_LABELS_CACHE_TTL_SEC` or redeploy for immediate JSONL-only fallback.

## Revoke a label

```sql
DELETE FROM solrisk_wallet_labels
WHERE wallet_pubkey = '<base58>' AND source = 'internal' AND label = 'phishing_drainer';
```

## Moderation from scam reports

User reports land in `solrisk_scam_reports` (`status = 'pending'`). After review:

1. Promote to `solrisk_wallet_labels` with `source = 'user_report'`
2. Mark report `status = 'approved'`

## Discovery

Buyers can check coverage without paying:

- `GET /health` → `label_coverage`
- `GET /api/v1/subscribe/info` → `label_coverage`
