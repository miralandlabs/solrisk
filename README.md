# solrisk — Solana Risk Scoring (x402 v2)

Dual-mode x402 seller: **per-call** micropayments **and** **subscription JWT** on the same data routes.

**Production status (v0.2.1):** **wallet-risk** and **subscription** are production-ready for buyer agents — curated deny/allow labels (6,900+ / 20+), honest scoring (v1.1.1), and machine-action fields (`recommendation`, `cache_hit`, `cluster`). **token-risk** is **beta** (on-chain mint/holder signals only; LP and deployer depth are P1). **tx-risk** is **reserved** — route returns **501** and is not billed (see below).

## Endpoints

| Route | Status | Auth | Description |
|-------|--------|------|-------------|
| `GET /api/v1/wallet-risk?wallet=` | **Production** | Bearer **or** x402 | Wallet screening (scoring v1.1.1) |
| `GET /api/v1/token-risk?mint=` | **Beta** | Bearer **or** x402 | Token rug-pull heuristics |
| `GET /api/v1/tx-risk?signature=` | **Reserved** | — | **501** — not a product SKU; planned P1 |
| `POST /api/v1/subscribe?tier=` | **Production** | x402 only | Issue subscription JWT |
| `GET /api/v1/subscribe/info` | **Production** | none | Tier catalog + label coverage |

## Pricing (mainnet seeds)

| SKU | Per-call | Subscribe hourly |
|-----|----------|------------------|
| wallet-risk | $0.05 | — |
| token-risk | $0.10 | — |
| all routes (bundle) | — | $1.00 / $5.00 daily / $25.00 monthly |

Preview uses `migrations/parameters-seed-devnet.sql` (lower subscribe prices).

## Setup

1. Copy `env.example` → `.env` (`X402_*`, `RPC_URL`, `JWT_SECRET` for subscription).
2. Run `migrations/init.sql` (complete v2 schema).
3. Seed pricing: `parameters-seed-devnet.sql` or `parameters-seed-mainnet.sql`.
4. Seed labels: `migrations/labels-seed.sql` (or regenerate via `python3 scripts/build_label_seeds.py`).
5. `vercel deploy`

See [migrations/CUTOVER.md](migrations/CUTOVER.md) and [migrations/LABELS.md](migrations/LABELS.md).

## tx-risk (reserved, not sunset)

The `/api/v1/tx-risk` route stays in the API as a **reserved namespace** — it returns **501 before auth** so nothing is charged. We keep it (rather than remove the route) because:

- Transaction screening is a distinct buyer workflow from wallet screening (pre-sign transfer review).
- A stable URL lets agents probe once and cache “not yet available” without a breaking 404 later.
- Implementation is planned for P1 (`getParsedTransaction`); removing the route would force a version bump when it ships.

If tx-risk is deprioritized entirely, sunset by removing the handler, `vercel.json` route, and OpenAPI path — not before that decision.

## Verify

```bash
cargo fmt --all -- --check
cargo clippy --bin risk_api -- -D warnings
cargo test --lib
cargo build --bin risk_api
```

## x402 Ecosystem

Part of [miraland-labs/x402](https://github.com/miraland-labs/x402). Dual-mode reference alongside [SUBSCRIPTION_PATTERN.md](../SUBSCRIPTION_PATTERN.md).
