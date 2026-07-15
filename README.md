# solrisk — Solana Risk Scoring (x402 v2)

Dual-mode x402 seller: **per-call** micropayments **and** **subscription JWT** on the same data routes.

**Production status (v0.3.0).** All SKUs are live for buyer agents, with honest scoring
(unmeasured signals are `null`, never synthesized) and machine-action fields
(`recommendation`, `cache_hit`, `cluster`):

- **wallet-risk** — screening + real multi-hop fund-flow provenance and recent-counterparty exposure (scoring v1.3.0).
- **tx-risk** — pre-sign transaction screening: `SIGN / REVIEW / BLOCK` *before* the agent signs (scoring v2.1.0).
- **token-risk** (beta) — on-chain rug heuristics (mint/holder only; LP + deployer depth are P2).
- **subscription** — JWT tiers across all routes.

Payment settles before RPC work (blockhash expiry makes verify→serve→settle unsafe); on a
post-payment failure the `503` carries the settlement proof (`PAYMENT-RESPONSE` header +
`settlement_sig`) for reconciliation. Curated deny/allow labels: 6,900+ / 20+. See
[docs/ENHANCEMENT_PLAN.md](docs/ENHANCEMENT_PLAN.md) for the roadmap.

## Endpoints

| Route | Status | Auth | Description |
|-------|--------|------|-------------|
| `GET /api/v1/wallet-risk?wallet=` | **Production** | Bearer **or** x402 | Wallet screening + real fund-flow trace & counterparty exposure (scoring v1.3.0) |
| `GET /api/v1/token-risk?mint=` | **Beta** | Bearer **or** x402 | Token rug-pull heuristics |
| `GET /api/v1/tx-risk?transaction=[&owner=]` | **Production** | Bearer **or** x402 | Pre-sign screening → `SIGN`/`REVIEW`/`BLOCK` (scoring v2.1.0) |
| `POST /api/v1/subscribe?tier=` | **Production** | x402 only | Issue subscription JWT |
| `GET /api/v1/subscribe/info` | **Production** | none | Tier catalog + label coverage |

## Pricing (mainnet seeds)

| SKU | Per-call | Subscribe hourly |
|-----|----------|------------------|
| wallet-risk | $0.05 | — |
| token-risk | $0.10 | — |
| tx-risk (pre-sign) | $0.25 | — |
| all routes (bundle) | — | $1.00 / $5.00 daily / $25.00 monthly |

Preview uses `migrations/parameters-seed-devnet.sql` (lower subscribe prices).

## Setup

1. Copy `env.example` → `.env` (`X402_*`, `RPC_URL`, `JWT_SECRET` for subscription).
2. Run `migrations/init.sql` (complete v2 schema).
3. Seed pricing: `parameters-seed-devnet.sql` or `parameters-seed-mainnet.sql`.
4. Seed labels: `migrations/labels-seed.sql` (or regenerate via `python3 scripts/build_label_seeds.py`).
5. `vercel deploy`

See [migrations/CUTOVER.md](migrations/CUTOVER.md) and [migrations/LABELS.md](migrations/LABELS.md).

## wallet-risk (screening + fund-flow)

`GET /api/v1/wallet-risk?wallet=<pubkey>` scores a wallet 0–100
(`LOW`/`MEDIUM`/`HIGH`/`CRITICAL`) → `ALLOW`/`REVIEW`/`BLOCK`, combining curated deny/allow
labels with on-chain signals and real fund-flow analysis:

- **Provenance** — traces the wallet's funder chain back up to 3 hops. A deny-labeled source
  flags `FUNDED_BY_LABELED` (direct) or `FUNDING_CHAIN_LABELED:hop{n}` (deeper, weighted by
  distance).
- **Counterparty exposure** — parses recent (30d) txns for real `unique_counterparties_30d`
  and flags recent dealings with deny-labeled addresses (`COUNTERPARTY_LABELED`).

Fund-flow work is bounded (env-tunable) and best-effort: anything it can't establish is
reported (`partial_history`, `null` metrics), never guessed.

## tx-risk (pre-sign screening)

`GET /api/v1/tx-risk?transaction=<base64 unsigned tx>[&owner=<pubkey>]` returns a
`SIGN`/`REVIEW`/`BLOCK` verdict *before* the agent signs — the highest-value pre-flight
decision in the agentic economy ("should I sign this?"). Subject defaults to the fee payer;
override with `owner`.

**Static screen (deterministic, no RPC).** Once the tx decodes, the base verdict never
depends on a round-trip:

- **`BLOCK`** — a deny-listed program (known drainer/scam), or an SPL Token `SetAuthority`
  handoff (owner/close/mint/freeze authority change).
- **`REVIEW`** — SPL Token `Approve` delegation, `CloseAccount`, an unlabeled program, or a
  v0 tx hiding programs behind an address lookup table (reduced visibility — never assumed safe).
- **`SIGN`** — only well-known, benign programs (System / Token / ATA / ComputeBudget / Memo).

**Simulation (best-effort enrichment).** A `simulateTransaction` pass adds disclosure:
`WOULD_FAIL` (the tx would revert) and the subject's net **SOL** and **SPL-token** balance
changes (`net_sol_change_lamports`, `net_token_changes`). An outflow is escalated **only**
when value leaves through an opaque/unlabeled program (`OUTFLOW_VIA_UNKNOWN_PROGRAM` /
`TOKEN_OUTFLOW_VIA_UNKNOWN_PROGRAM`) — a legitimate send is never flagged. If the RPC is
unavailable, the deterministic static verdict still stands (`simulated: false`).

**Contract.** A non-base64 `transaction` is rejected before payment (`400`). A paid request
whose input is valid base64 but not a decodable transaction returns `422` with the
settlement proof. Token-2022 balance deltas are a follow-up (see the roadmap).

## Verify

```bash
cargo fmt --all -- --check
cargo clippy --bin risk_api -- -D warnings
cargo test --lib
cargo build --bin risk_api
```

## x402 Ecosystem

Part of [miraland-labs/x402](https://github.com/miraland-labs/x402). Dual-mode reference alongside [SUBSCRIPTION_PATTERN.md](../SUBSCRIPTION_PATTERN.md).
