# solrisk — Solana Risk Scoring (x402 v2)

Dual-mode x402 seller: **per-call** micropayments **and** **subscription JWT** on the same data routes.

## Endpoints

| Route | Auth | Description |
|-------|------|-------------|
| `GET /api/v1/wallet-risk?wallet=` | Bearer **or** x402 | Wallet risk (scoring v1.1.0) |
| `GET /api/v1/token-risk?mint=` | Bearer **or** x402 | Token rug-pull risk |
| `GET /api/v1/tx-risk?signature=` | Bearer **or** x402 | Transaction risk |
| `POST /api/v1/subscribe?tier=` | x402 only | Issue subscription JWT |
| `GET /api/v1/subscribe/info` | none | Tier catalog |

## Pricing (mainnet seeds)

| SKU | Per-call | Subscribe hourly |
|-----|----------|------------------|
| wallet-risk | $0.05 | — |
| token-risk | $0.10 | — |
| tx-risk | $0.05 | — |
| all routes (bundle) | — | $1.00 / $5.00 daily / $25.00 monthly |

Preview uses `migrations/parameters-seed-devnet.sql` (lower subscribe prices).

## Setup

1. Copy `env.example` → `.env` (`X402_*`, `RPC_URL`, `JWT_SECRET` for subscription).
2. Run `migrations/init.sql` (complete v2 schema).
3. Seed pricing: `parameters-seed-devnet.sql` or `parameters-seed-mainnet.sql`.
   (Only run `002_parameters_v2.sql` when upgrading an existing v0.1 database.)
4. `vercel deploy`

See [migrations/CUTOVER.md](migrations/CUTOVER.md) for shared Supabase cutover.

## Verify

```bash
cargo fmt --all -- --check
cargo clippy --bin risk_api -- -D warnings
cargo test --lib
cargo build --bin risk_api
```

## x402 Ecosystem

Part of [miraland-labs/x402](https://github.com/miraland-labs/x402). Dual-mode reference alongside [SUBSCRIPTION_PATTERN.md](../SUBSCRIPTION_PATTERN.md).
