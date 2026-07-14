# solrisk — Solana Risk Scoring (x402 v2)

Dual-mode x402 seller: **per-call** micropayments **and** **subscription JWT** on the same data routes.

**Production status (v0.3.0):** **wallet-risk** and **subscription** are production-ready for buyer agents — curated deny/allow labels (6,900+ / 20+), honest scoring (v1.2.0; unmeasured signals are `null`, never synthesized), and machine-action fields (`recommendation`, `cache_hit`, `cluster`). Payment settles before RPC work (Solana blockhash expiry makes verify→serve→settle unsafe); if scoring then fails, the 503 carries the settlement proof (`PAYMENT-RESPONSE` header + `settlement_sig`) for reconciliation. **tx-risk** is now a **live SKU** — **pre-sign transaction screening**: an agent submits a base64 unsigned transaction and gets a `SIGN / REVIEW / BLOCK` verdict *before signing*. v1 is deterministic static instruction analysis (no RPC), so once the tx decodes the verdict never depends on a round-trip; balance-delta drain detection via `simulateTransaction` is the next slice. **token-risk** is **beta** (on-chain mint/holder signals only; LP and deployer depth are P1). See [docs/ENHANCEMENT_PLAN.md](docs/ENHANCEMENT_PLAN.md) for the roadmap.

## Endpoints

| Route | Status | Auth | Description |
|-------|--------|------|-------------|
| `GET /api/v1/wallet-risk?wallet=` | **Production** | Bearer **or** x402 | Wallet screening (scoring v1.2.0) |
| `GET /api/v1/token-risk?mint=` | **Beta** | Bearer **or** x402 | Token rug-pull heuristics |
| `GET /api/v1/tx-risk?transaction=[&owner=]` | **Production** | Bearer **or** x402 | Pre-sign screening → `SIGN`/`REVIEW`/`BLOCK` (scoring v2.0.0) |
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

## tx-risk (pre-sign screening)

`GET /api/v1/tx-risk?transaction=<base64 unsigned tx>[&owner=<pubkey>]` returns a
`SIGN / REVIEW / BLOCK` verdict for a transaction **before** the agent signs it — the
highest-value pre-flight decision in the agentic economy ("should I sign this?").

**v1 (deterministic, no RPC).** The static instruction decode flags:

- **`BLOCK`** — a program on the **deny list** (known drainer/scam), or an SPL Token
  **`SetAuthority`** handoff (owner/close/mint/freeze authority change).
- **`REVIEW`** — SPL Token **`Approve`** delegation, **`CloseAccount`**, an **unlabeled
  program**, or a v0 tx that hides programs behind an **address lookup table** (reduced
  static visibility — never assumed safe).
- **`SIGN`** — only well-known, benign programs (System / Token / ATA / ComputeBudget / Memo).

Because the static base is pure-CPU, the verdict is deterministic once the tx decodes —
it never depends on an RPC round-trip. Subject defaults to the fee payer; override with `owner`.

**v1.1 — simulation enrichment (best-effort).** On top of the static base, a
`simulateTransaction` pass discloses whether the tx would revert (`WOULD_FAIL`) and the
subject's net SOL change (`net_sol_change_lamports`). A simulated SOL outflow is escalated
**only** when it leaves the subject *through an opaque/unlabeled program*
(`OUTFLOW_VIA_UNKNOWN_PROGRAM`) — a legitimate send to known programs is never flagged. If
the RPC/sim is unavailable, the deterministic static verdict still stands (`simulated:
false`). Token-balance deltas are the v1.2 slice.

**Contract notes.** A non-base64 `transaction` is rejected **before** payment (`400`). A
paid request whose input is valid base64 but not a decodable transaction returns `422`
**with** the settlement proof for reconciliation. Balance-delta drain detection via
`simulateTransaction` is the documented v1.1 slice — see
[docs/ENHANCEMENT_PLAN.md](docs/ENHANCEMENT_PLAN.md).

## Verify

```bash
cargo fmt --all -- --check
cargo clippy --bin risk_api -- -D warnings
cargo test --lib
cargo build --bin risk_api
```

## x402 Ecosystem

Part of [miraland-labs/x402](https://github.com/miraland-labs/x402). Dual-mode reference alongside [SUBSCRIPTION_PATTERN.md](../SUBSCRIPTION_PATTERN.md).
