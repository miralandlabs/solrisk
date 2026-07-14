# solrisk — Enhancement Plan (v0.3.0 → )

**Goal:** move solrisk from selling commodity stats (~$0.05) to selling **verdicts backed by
signals an agent can't cheaply self-compute** — so each call genuinely deserves **≥ $0.25**.

## Value principle
An agent can `getBalance`/`getSignatures` itself for free. It *cannot* cheaply: simulate a
transaction for drainers, trace fund flows across hops, assess LP/deployer rug depth, or know
curated labels. **Charge for the non-DIY-able verdict, not the primitive.** Keep the
"measured-or-null, never synthesized" integrity — it is solrisk's credibility with agents.

## Roadmap (prioritized by value-per-effort)

### P0 — `tx-risk` as pre-sign screening  ← the categorical jump (in progress, v0.3.0)
Turn the reserved `501` route into a real SKU. Input a base64 **unsigned** transaction; return
`SIGN / REVIEW / BLOCK` **before the agent signs**. Moves solrisk from "reputation lookup"
(nice-to-have) to "loss prevention" (must-have). Worth **$0.25–$0.50**; needs zero label coverage.

- **v1 (this release) — deterministic static instruction screening** (no simulation; reliable):
  decode the tx, and per instruction flag —
  - program id on the **deny list** (known drainer/scam program) → `BLOCK`
  - SPL Token **SetAuthority** (owner/close/mint authority handoff) → `BLOCK`
  - SPL Token **Approve/ApproveChecked** (delegation to a non-system delegate) → `REVIEW`
  - **CloseAccount** with rent to a third party → `REVIEW`
  - unknown/unlabeled program touched → contributes to `REVIEW`
  - subject = fee payer (or `?owner=` override); honest `simulated: false`.
- **v1.1 — simulation enrichment (shipped, v0.3.0):** best-effort `simulateTransaction`
  discloses `WOULD_FAIL` (tx reverts) and the subject's **net SOL change**; a simulated
  outflow *through an opaque program* escalates to REVIEW (`OUTFLOW_VIA_UNKNOWN_PROGRAM`),
  while legitimate sends are never flagged. RPC-unavailable falls back to the static verdict.
- **v1.2 — token-balance deltas (shipped, v0.3.0):** simulation also measures the subject's
  SPL-token balance changes (bounded to the token accounts the tx touches) →
  `net_token_changes`. A token outflow *through an opaque program* escalates
  (`TOKEN_OUTFLOW_VIA_UNKNOWN_PROGRAM`); a legitimate token send is not. Token-2022 accounts
  are the next follow-up.
- Input: `?transaction=<base64>` (pre-sign, primary). `?signature=` (post-hoc forensics via
  `getParsedTransaction`) is a later, lower-value slice.

### P1 — make `funding_source_risk` real (fund-flow graph)
The old `funding_source_from_sigs(tx_count, age_days, first_seen_ts)` heuristic **traced
nothing**. This is the AML moat. **Lifts wallet-risk → $0.25.**

- **P1 v1 — direct funder trace (shipped, v0.3.0):** once signature pagination reaches the
  wallet's **genesis** tx, fetch it, extract the original funder (largest SOL sender to the
  wallet in its first tx), and deny-label-check them. Real classifications replace the
  heuristic: `no_history` | `partial_history` | `genesis_untraceable` | `labeled_bad` |
  `traced_clean`; a deny-labeled funder adds `FUNDED_BY_LABELED`. Honest: partial history or
  an unmappable (lookup-table) genesis is reported, never guessed.
- **P1.2 — counterparty exposure (shipped, v0.3.0):** parses a bounded window of recent
  (30d) txns (env `SOLRISK_MAX_TX_PARSE`, default 20) for real `unique_counterparties_30d`
  + `program_diversity_30d` (no longer `null`) and deny-labeled counterparty exposure
  (`COUNTERPARTY_LABELED` +30 — "who does this wallet deal with"). Best-effort; a tx that
  can't be mapped (lookup tables) is skipped, not guessed. Scoring → v1.3.0.
- **P1.1 — multi-hop:** walk 2–3 hops back from the funder and label-check each.
- **Follow-ups:** parallelize the P1.2 getTransaction fan-out (currently sequential,
  bounded); Token-2022 in the tx-risk sim; `% inflow from labeled-bad` weighting.

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
| SKU | v0.2.2 | Target | Justification |
|---|---|---|---|
| tx-risk (pre-sign) | 501 | **$0.25–$0.50** | prevents direct loss; can't DIY |
| wallet-risk | $0.05 | $0.25 | real fund-flow graph + labels |
| token-risk | $0.10 | $0.25 | LP + deployer + honeypot |
| signed attestation | — | premium | reusable proof-of-diligence |
| batch / watchlist | — | subscription | value-dense / recurring |

## Guardrails
- **Measured-or-null**, never synthesized. A pre-sign verdict must never claim certainty it can't back.
- **Settle-before-serve**: RPC/sim work runs after payment (blockhash expiry); on failure return the
  `503` with settlement proof for reconciliation (existing pattern). Cap sim/hops; cache; reuse `rpc_retry`.
- Everything **additive** to the public contract; existing response fields stay.

## Sequence
P0 (tx-risk) → P1 (fund-flow) deliver the biggest agent value and justify the repricing; P2–P5 compound it.
