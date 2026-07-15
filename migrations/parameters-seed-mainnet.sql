-- solrisk v2 mainnet production pricing seeds
-- Run on production Supabase after 002_parameters_v2.sql

INSERT INTO parameters (service, endpoint, param_name, param_value, inactive)
VALUES
  ('solrisk', 'wallet-risk', 'X402_ACCEPTS_JSON', '[{"kind":"usdc","amountUi":"0.25"}]', false),
  ('solrisk', 'token-risk', 'X402_ACCEPTS_JSON', '[{"kind":"usdc","amountUi":"0.10"}]', false),
  ('solrisk', 'tx-risk', 'X402_ACCEPTS_JSON', '[{"kind":"usdc","amountUi":"0.30"}]', false),
  ('solrisk', '/api/v1/subscribe/hourly', 'X402_ACCEPTS_JSON', '[{"kind":"usdc","amountUi":"1.00"}]', false),
  ('solrisk', '/api/v1/subscribe/daily', 'X402_ACCEPTS_JSON', '[{"kind":"usdc","amountUi":"5.00"}]', false),
  ('solrisk', '/api/v1/subscribe/monthly', 'X402_ACCEPTS_JSON', '[{"kind":"usdc","amountUi":"25.00"}]', false)
ON CONFLICT (service, endpoint, param_name) DO UPDATE SET
  param_value = EXCLUDED.param_value,
  updated_at = NOW();
