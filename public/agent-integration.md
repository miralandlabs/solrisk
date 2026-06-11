# solrisk v2 — Agent Integration

solrisk is a **production-ready dual-mode x402 seller** on the `exact` rail for **wallet screening** and **subscription polling**. Pay per call **or** subscribe once and use `Authorization: Bearer` on paid data routes.

## What to use in production

| Route | Status | Use for |
|-------|--------|---------|
| `GET /api/v1/wallet-risk` | **Production** | Screen wallets before transfer, payout, or custody |
| `POST /api/v1/subscribe` + Bearer | **Production** | High-volume polling without per-call x402 |
| `GET /api/v1/token-risk` | **Beta** | Mint authority / holder concentration heuristics only |
| `GET /api/v1/tx-risk` | **Reserved** | Do **not** integrate — always **501**, not billed |

## Data routes (dual auth)

| Route | Query | Per-call (mainnet) |
|-------|-------|-------------------|
| `GET /api/v1/wallet-risk` | `wallet=` | $0.05 USDC |
| `GET /api/v1/token-risk` | `mint=` | $0.10 USDC |

**Auth (pick one):**

1. `Authorization: Bearer <jwt>` — subscription (no per-call x402)
2. `PAYMENT-SIGNATURE` — per-call x402 proof

Without either → HTTP **402** with per-endpoint `accepts[]` and `extensions.subscribeUrl`.

## Subscription flow

1. `GET /api/v1/subscribe/info` — tier catalog, `label_coverage`, `persistenceHint`
2. `POST /api/v1/subscribe?tier=hourly|daily|monthly` without payment → **402 JSON body** (no `Payment-Required` header)
3. Pay via x402, retry with `PAYMENT-SIGNATURE` → receive JWT
4. Save token locally until `expiresAt`
5. Use `Authorization: Bearer <token>` on wallet-risk and token-risk

### Mainnet subscription pricing

| Tier | Price | Window |
|------|-------|--------|
| hourly | $1.00 | 1 hour |
| daily | $5.00 | 24 hours |
| monthly | $25.00 | 30 days |

## Response envelope (wallet-risk, token-risk)

```json
{
  "api_version": 2,
  "scoring_version": "1.1.1",
  "recommendation": "ALLOW",
  "signal_quality": "medium",
  "cluster": "mainnet",
  "cache_hit": false,
  "checked_at": "2026-06-10T12:00:00Z"
}
```

| Field | Values | Meaning |
|-------|--------|---------|
| `recommendation` | `ALLOW`, `REVIEW`, `BLOCK` | Machine action hint |
| `cache_hit` | bool | `true` if served from 5-min cache |
| `cached_at` | ISO8601 | Present when `cache_hit` is true |
| `cluster` | `mainnet`, `devnet` | From seller `X402_NETWORK` |

Wallet `scoring_version` **1.1.1** — activity bonus no longer uses estimated counterparty metrics.

## Label coverage (free)

`GET /health` and `GET /api/v1/subscribe/info` return `label_coverage` with deny/allow counts.

## Error codes (data routes)

| Code | HTTP | Meaning |
|------|------|---------|
| `TOKEN_EXPIRED` | 401 | Renew via `/subscribe` |
| `TOKEN_REVOKED` | 401 | Subscription revoked |
| `SUBSCRIBER_RATE_LIMIT_EXCEEDED` | 429 | Per-payer fair use |
| `RATE_LIMIT_EXCEEDED` | 429 | Global per-IP limit |
| `NOT_IMPLEMENTED` | 501 | tx-risk reserved — ignore in production integrations |

## tx-risk

Do not call `/api/v1/tx-risk` in production agents. The route exists as a reserved endpoint (501 before auth, no x402 charge) for a future P1 implementation. Wallet screening covers the primary agent use case today.

## Discovery

- `GET /.well-known/x402-resources.json` — payable resources (wallet, token, subscribe tiers)
- `GET /openapi.json` — OpenAPI 0.2.1
