# solrisk — Enhancement Plan (v0.3.0 → )

**Goal:** move solrisk from selling commodity stats (~$0.05) to selling **verdicts backed by
signals an agent can't cheaply self-compute** — so each call genuinely deserves **≥ $0.25**.

## Value principle
An agent can `getBalance`/`getSignatures` itself for free. It *cannot* cheaply: simulate a
transaction for drainers, trace fund flows across hops, assess LP/deployer rug depth, or know
curated labels. **Charge for the non-DIY-able verdict, not the primitive.** Keep the
"measured-or-null, never synthesized" integrity — it is solrisk's credibility with agents.

## Shipped (v0.3.0)

- **tx-risk — pre-sign screening** (new SKU, $0.25; scoring v2.1.0). Returns
  `SIGN`/`REVIEW`/`BLOCK` before an agent signs — "loss prevention", not "reputation lookup".
  Deterministic static verdict (deny-listed programs & SPL Token `SetAuthority` → `BLOCK`;
  delegations, closes, unlabeled programs, lookup-table visibility → `REVIEW`) plus
  best-effort `simulateTransaction` disclosure (`WOULD_FAIL`, net SOL & SPL-token balance
  changes), escalating **only** value that leaves through an opaque program.
- **wallet-risk — real fund-flow** (scoring v1.3.0), replacing the
  `funding_source_from_sigs` heuristic that traced nothing:
  - **Multi-hop provenance** — traces the funder chain up to `SOLRISK_MAX_FUNDER_HOPS`
    (default 3); a deny-labeled source → `FUNDED_BY_LABELED` (direct) or
    `FUNDING_CHAIN_LABELED:hop{n}` (deeper, weighted by distance).
  - **Counterparty exposure** — parses recent (30d) txns (`SOLRISK_MAX_TX_PARSE`, default 20)
    for real `unique_counterparties_30d` + `program_diversity_30d` and `COUNTERPARTY_LABELED`.
  - Bounded + best-effort; anything it can't establish is reported (`partial_history`,
    `null`), never guessed.

## Roadmap

**Near-term follow-ups**
- Token-2022 balance deltas in the tx-risk sim.
- Parallelize the wallet-risk `getTransaction` fan-out (sequential + bounded today) — latency.
- `% inflow from labeled-bad` weighting; optional `?signature=` post-hoc tx forensics.

### P2 — complete `token-risk` rug depth
Add LP pool discovery + **lock/burn** status + depth-vs-mcap; **deployer history** (serial-rugger
detection); **sellability/honeypot** simulation. Keep unmeasured = null. **Lifts token-risk → $0.25.**

### P3 — verdict packaging + auditability
Reshape responses into a decision object: `{ recommendation, confidence, verdict_ttl,
top_reasons:[{code,severity,evidence}], scored_at }`. Add an optional **signed attestation**
(reusable proof-of-diligence — itself a sellable x402 artifact).

### P4 — in-network reputation moat (unique to solrisk)
solrisk sits inside the payment network. Wire in **pr402-registry** seller/buyer history (on-time
delivery, dispute/refund rate) → counterparty reputation no external risk API has. Add
reputation-weighted crowdsourced scam reporting. Labels + network reputation compound with usage.

### P5 — recurring / value-dense SKUs
Batch scoring (N subjects per call), watchlists + webhooks ("alert if this wallet receives from a
mixer / crosses HIGH") → subscription. Raises ARPU and stickiness.

## Pricing map
| SKU | Now | Note |
|---|---|---|
| tx-risk (pre-sign) | **$0.25** (shipped) | prevents direct loss; can't DIY |
| wallet-risk | $0.05 | fund-flow now real → reprice to $0.25 |
| token-risk | $0.10 (beta) | P2 (LP + deployer) to justify $0.25 |
| signed attestation | — (P3) | reusable proof-of-diligence |
| batch / watchlist | — (P5) | value-dense / recurring |

## Guardrails
- **Measured-or-null**, never synthesized. A pre-sign verdict must never claim certainty it can't back.
- **Settle-before-serve**: RPC/sim work runs after payment (blockhash expiry); on failure return the
  `503` with settlement proof for reconciliation (existing pattern). Cap sim/hops; cache; reuse `rpc_retry`.
- Everything **additive** to the public contract; existing response fields stay.

## Sequence
tx-risk + fund-flow (shipped) delivered the biggest agent value. Next: reprice wallet-risk,
then P2 (token-risk depth) → P3–P5 compound it.
