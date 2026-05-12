# solrisk | Wallet Risk Scoring — Agent Integration

solrisk is an **x402-enforced resource provider** that answers one question: *"Is this Solana wallet safe to transact with?"*

## Endpoint

```
GET /api/v1/wallet-risk?wallet=<base58_pubkey>
```

## Payment

$0.05 USDC per call via x402 v2. Include `PAYMENT-SIGNATURE` header (raw JSON or base64). Every call requires payment.

## Response shape (200 OK)

```json
{
  "wallet": "Ac2ev4ofDx61tuSJCgq9ToSBfVwHD2a1FVJf6p7TAqiB",
  "risk_score": 18,
  "risk_band": "LOW",
  "checked_at": "2026-05-12T04:17:00Z",
  "signals": {
    "age_days": 412,
    "tx_count_30d": 87,
    "tx_count_total": 1204,
    "unique_counterparties_30d": 34,
    "sol_balance_lamports": 218400000,
    "spl_account_count": 12,
    "has_activity_48h": true,
    "program_diversity_30d": 9,
    "is_fresh_funded": false,
    "funding_source_risk": "not_checked"
  },
  "flags": [],
  "labels": [],
  "confidence": 0.82,
  "scoring_version": "1.0.0"
}
```

## Risk bands

| Score | Band | Meaning |
|---|---|---|
| 0–24 | `LOW` | Safe to transact at face value |
| 25–49 | `MEDIUM` | Proceed with caution |
| 50–74 | `HIGH` | Manual review recommended |
| 75–100 | `CRITICAL` | Refuse or escalate |

## Error codes

| Code | HTTP | Meaning |
|---|---|---|
| `BAD_REQUEST` | 400 | Missing or invalid `wallet` param |
| `PAYMENT_REQUIREMENTS_UNAVAILABLE` | 503 | Pricing config error |
| `NOT_IMPLEMENTED` | 501 | Scoring engine not yet deployed (MVP stub) |

## 402 flow

Same as any x402 v2 seller:

1. Call without `PAYMENT-SIGNATURE` → get 402 with `Payment-Required` header (base64 JSON).
2. Decode `accepts[]`, build tx via pr402 facilitator `/build-exact-payment-tx`.
3. Sign, then retry with `PAYMENT-SIGNATURE` header.

## Discovery

- OpenAPI: `/openapi.json`
- x402 manifest: `/.well-known/x402.json`
- Health: `/health`

## Limitations (v1)

- Solana only. No cross-chain tracing.
- Label database is curated and growing; coverage improves over time.
- `confidence` reflects data coverage, not prediction accuracy.
- Scores are versioned (`scoring_version`). Re-fetch if you persist them.
