# AGENTS.md

This file is for AI agents (Cursor, Claude Code, etc.), not human developers.
Philosophy: **Simple is Best, yet Elegant.** Make the smallest change that solves
the task; do not refactor, abstract, or add features that were not asked for.

`solrisk` is a **paid x402 seller**: a Solana wallet risk-scoring API that gates one
HTTP route with `402 Payment Required` (scheme `exact`) and settles via a pr402
facilitator. It is a deployed production service — read and extend it, don't grow it
into a framework.

## Topology

Single Rust crate (`solrisk`), bin **`risk_api`**, deployed on Vercel (`vercel.json`).
`data/`, `migrations/` exist but the service runs **stateless by default**; Postgres
(`DATABASE_URL`) is optional and only stores tunable parameters.

The paid endpoint is **`GET /api/v1/wallet-risk?wallet=<pubkey>`**.

## Hard boundaries (do not cross without explicit human approval)

- **Gate exactly one route** (`GET /api/v1/wallet-risk`, scheme `exact`). Don't add a
  registry, second paywall, or framework abstractions.
- **Stay stateless-capable.** Don't make Postgres required; DB is parameters-only.
- **An unpaid request must answer with HTTP `402`** carrying payment terms — the pr402
  discovery probe depends on it. The SRM `resourceUrl` carries a sample `?wallet=` so the
  probe reaches the 402 gate instead of a `400` input error; keep that sample arg.
- **Env var names are a contract** shared with sibling x402 services: `X402_FACILITATOR_URL`,
  `X402_PAY_TO`, `X402_NETWORK`, `X402_SCHEME`, `X402_PAYMENT_AMOUNT_USDC`,
  `X402_PAYMENT_TIMEOUT_SECONDS`, `RPC_URL` (optional `X402_MERCHANT_WALLET`). RPC resilience
  knobs are `SOLRISK_RPC_*`. Don't rename.
- **Authoritative payment terms = the live HTTP 402.** `/.well-known/x402-resources.json`
  (the SRM) is advisory discovery metadata only; its `resourceUrl` host must equal the
  service origin (origin binding).
- **No new dependencies** unless asked.

## Verify before claiming done (fix, don't suppress)

```bash
cargo fmt --all -- --check
cargo clippy --bin risk_api -- -D warnings
cargo test --lib
cargo build --bin risk_api
```
