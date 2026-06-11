# AGENTS.md

This file is for AI agents (Cursor, Claude Code, etc.), not human developers.
Philosophy: **Simple is Best, yet Elegant.** Make the smallest change that solves
the task; do not refactor, abstract, or add features that were not asked for.

`solrisk` v2 is the **canonical open-source x402 seller** on the `exact` rail:
per-call payment **and** subscription JWT on the same data routes (dual auth).

## Topology

Single Rust crate (`solrisk`), bin **`risk_api`**, deployed on Vercel (`vercel.json`).
Postgres (`DATABASE_URL`) is **optional** — env vars always work; DB enables per-endpoint
pricing, subscriptions, labels, cache, audit log, and rate limits.

### Paid per-call routes (x402 `PAYMENT-SIGNATURE` or Bearer JWT)

| Endpoint key | Route |
|--------------|-------|
| `wallet-risk` | `GET /api/v1/wallet-risk?wallet=` |
| `token-risk` | `GET /api/v1/token-risk?mint=` |
| `tx-risk` | `GET /api/v1/tx-risk?signature=` |

### Subscription routes (x402 gate only here)

| Endpoint key | Route |
|--------------|-------|
| `subscribe-hourly` | `POST /api/v1/subscribe?tier=hourly` |
| `subscribe-daily` | `POST /api/v1/subscribe?tier=daily` |
| `subscribe-monthly` | `POST /api/v1/subscribe?tier=monthly` |

One JWT covers **all** data routes until `exp`. See [SUBSCRIPTION_PATTERN.md](../SUBSCRIPTION_PATTERN.md).

### Dual auth on data routes

1. `Authorization: Bearer <jwt>` → verify + revocation + per-payer rate limit → score (no x402)
2. `PAYMENT-SIGNATURE` → per-endpoint x402 gate → score
3. Neither → HTTP 402 with per-endpoint `accepts[]` (+ subscribe hint)

## Hard boundaries

- **Each paid route maps to one `endpoint` key** in `parameters` (mintforge / spl-balance pattern).
  No generic payment framework — explicit catalog in `service_endpoints.rs`.
- **Stay env-capable.** Postgres enhances production; cold start without DB must still serve.
- **Subscribe 402:** JSON body only (no `Payment-Required` header). **Per-call 402:** keep header for backward compatibility.
- **Env var contract** unchanged: `X402_FACILITATOR_URL`, `X402_PAY_TO`, `X402_NETWORK`, `X402_SCHEME`,
  `X402_PAYMENT_AMOUNT_USDC`, `X402_PAYMENT_TIMEOUT_SECONDS`, `RPC_URL`, `JWT_SECRET` (subscription),
  `RATE_LIMIT_*`, `SOLRISK_RPC_*`.
- **SRM is advisory;** live 402 body is authoritative. Sample query args on `resourceUrl` for probe-friendly gates.
- **Approved v2 dependencies:** `jsonwebtoken` (subscription JWT).

## Verify before claiming done

```bash
cargo fmt --all -- --check
cargo clippy --bin risk_api -- -D warnings
cargo test --lib
cargo build --bin risk_api
```
