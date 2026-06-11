-- solrisk v2 devnet/preview pricing seeds
-- Run on preview Supabase after 002_parameters_v2.sql

INSERT INTO parameters (service, endpoint, param_name, param_value, inactive)
VALUES
  ('solrisk', 'wallet-risk', 'X402_ACCEPTS_JSON', '[{"kind":"usdc","amountUi":"0.05"}]', false),
  ('solrisk', 'token-risk', 'X402_ACCEPTS_JSON', '[{"kind":"usdc","amountUi":"0.05"}]', false),
  ('solrisk', 'tx-risk', 'X402_ACCEPTS_JSON', '[{"kind":"usdc","amountUi":"0.05"}]', false),
  ('solrisk', '/api/v1/subscribe/hourly', 'X402_ACCEPTS_JSON', '[{"kind":"usdc","amountUi":"0.05"}]', false),
  ('solrisk', '/api/v1/subscribe/daily', 'X402_ACCEPTS_JSON', '[{"kind":"usdc","amountUi":"0.30"}]', false),
  ('solrisk', '/api/v1/subscribe/monthly', 'X402_ACCEPTS_JSON', '[{"kind":"usdc","amountUi":"2.00"}]', false)
ON CONFLICT (service, endpoint, param_name) DO UPDATE SET
  param_value = EXCLUDED.param_value,
  updated_at = NOW();
