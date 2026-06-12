# Parameters v2 cutover (solrisk)

## Context

Shared Supabase may host `spl-token-balance`, `aethervane`, and `solrisk` rows.
spl-balance already uses `(service, endpoint, param_name)` — see its `migrations/init.sql`.

solrisk v2 aligns to the same shape. **Never delete or overwrite rows where `service != 'solrisk'`.**

## Fresh install (new / empty database)

1. Run `migrations/init.sql` — complete v2 schema (all tables).
2. Seed pricing:
   - Preview/devnet: `migrations/parameters-seed-devnet.sql`
   - Mainnet: `migrations/parameters-seed-mainnet.sql`
3. Seed labels: `migrations/labels-seed.sql` (or `python3 scripts/build_label_seeds.py`).
4. Set `JWT_SECRET` in Vercel (`openssl rand -hex 32`).
5. Set `DATABASE_URL` in Vercel and redeploy.

Skip `002_parameters_v2.sql` on a fresh database — it is only for upgrading legacy v0.1 installs.

## Upgrade existing v0.1 database

1. Run `migrations/002_parameters_v2.sql` on the shared database (idempotent).
2. Seed pricing:
   - Preview/devnet: `migrations/parameters-seed-devnet.sql`
   - Mainnet: `migrations/parameters-seed-mainnet.sql`
3. Seed labels: `migrations/labels-seed.sql`.
4. Set `JWT_SECRET` in Vercel (openssl rand -hex 32).
5. Deploy preview; verify `GET /api/v1/subscribe/info`, `label_coverage`, and per-endpoint 402 amounts.

## Legacy compatibility

During transition, solrisk still reads:

- `(service='solrisk', endpoint, param_name)` — preferred
- `(service='solrisk', endpoint='*', param_name)` — service-wide fallback
- Legacy `SOLRISK_*` param_name rows — deprecated

Env vars remain the final fallback when DB is unset or row missing.

## Rollback

Revert Vercel deployment to prior git tag. DB migrations are additive; no rollback SQL required for emergency.
