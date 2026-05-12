# solrisk — Solana Wallet Risk Scoring (x402)

Pay-per-call wallet risk scoring for Solana, using **HTTP 402** with **x402 v2** payloads and the **pr402** facilitator.

## What it does

One endpoint: `GET /api/v1/wallet-risk?wallet=<base58>`

Returns:
- **risk_score** (0–100)
- **risk_band** (LOW / MEDIUM / HIGH / CRITICAL)
- **signals** — chain-derived metrics (age, activity, counterparty diversity, dust patterns, funding source)
- **flags** — triggered risk indicators
- **labels** — matches from curated allow/deny lists (OFAC, Chainabuse, internal)
- **confidence** — data-coverage heuristic (not prediction accuracy)
- **scoring_version** — versioned formula for reproducibility

## Pricing

- **Paid:** $0.05 USDC per call via x402 v2 (`PAYMENT-SIGNATURE` header)

## Architecture

Same stack as `spl-token-balance-serverless`:
- Rust serverless on Vercel (`vercel-rust`)
- x402 payment gate (pr402 facilitator verify + settle)
- Shared Supabase DB (tables prefixed `solrisk_`; `parameters` table shared with `SOLRISK_` param_name prefix)

## Setup

1. Copy `env.example` to `.env` and fill in values.
2. Run `migrations/init.sql` against your Supabase DB.
3. Deploy: `vercel deploy`

## Shared DB discipline

This project shares a Supabase instance with `spl-token-balance-serverless`:
- **`parameters` table** is shared. solrisk uses `SOLRISK_*` param_name prefix; spl-balance uses `SPL_BALANCE_*`.
- **All other tables** use `solrisk_` prefix: `solrisk_wallet_labels`, `solrisk_scoring_log`, `solrisk_scam_reports`, `solrisk_score_cache`.
- **Different seller wallet + vault PDA** — separate row in pr402 `/providers`.

## x402 Ecosystem

Part of the [x402 ecosystem](https://github.com/miraland-labs/x402). Facilitator: [pr402](https://github.com/miralandlabs/pr402).
