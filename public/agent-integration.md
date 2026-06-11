# solrisk v2 — Agent Integration

solrisk is a **dual-mode x402 seller** on the `exact` rail: pay per call **or** subscribe once and use `Authorization: Bearer` on all data routes.

## Data routes (dual auth)

| Route | Query | Per-call (mainnet) |
|-------|-------|-------------------|
| `GET /api/v1/wallet-risk` | `wallet=` | $0.05 USDC |
| `GET /api/v1/token-risk` | `mint=` | $0.10 USDC |
| `GET /api/v1/tx-risk` | `signature=` | $0.05 USDC |

**Auth (pick one):**

1. `Authorization: Bearer <jwt>` — subscription (no per-call x402)
2. `PAYMENT-SIGNATURE` — per-call x402 proof

Without either → HTTP **402** with per-endpoint `accepts[]` and `extensions.subscribeUrl`.

## Subscription flow

1. `GET /api/v1/subscribe/info` — tier catalog + `persistenceHint`
2. `POST /api/v1/subscribe?tier=hourly|daily|monthly` without payment → **402 JSON body** (no `Payment-Required` header)
3. Pay via x402, retry with `PAYMENT-SIGNATURE` → receive JWT
4. Save token locally until `expiresAt` — seller does not re-issue without new payment
5. Use `Authorization: Bearer <token>` on wallet/token/tx routes

### Mainnet subscription pricing

| Tier | Price | Window |
|------|-------|--------|
| hourly | $1.00 | 1 hour |
| daily | $5.00 | 24 hours |
| monthly | $25.00 | 30 days |

Preview/devnet uses lower seed prices for integrator testing.

## Response envelope

All score endpoints include:

```json
{
  "api_version": 2,
  "scoring_version": "1.1.0",
  "signal_quality": "medium",
  "checked_at": "2026-06-10T12:00:00Z"
}
```

Wallet `scoring_version` is **1.1.0** (proxy counterparty metrics flagged via `counterparty_metrics_estimated`).

## Error codes (data routes)

| Code | HTTP | Meaning |
|------|------|---------|
| `TOKEN_EXPIRED` | 401 | Renew via `/subscribe` |
| `TOKEN_REVOKED` | 401 | Subscription revoked |
| `SUBSCRIBER_RATE_LIMIT_EXCEEDED` | 429 | Per-payer fair use |
| `RATE_LIMIT_EXCEEDED` | 429 | Global per-IP limit |

## Discovery

- `GET /.well-known/x402-resources.json` — dynamic multi-resource manifest
- `GET /openapi.json` — OpenAPI 0.2.0
