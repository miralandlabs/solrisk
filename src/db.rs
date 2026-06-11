//! Optional Postgres: parameters (v2 service/endpoint), subscriptions, rate limits, cache, audit.

use deadpool_postgres::{Client, Config, Pool, PoolConfig, Runtime};
use openssl::ssl::{SslConnector, SslMethod};
use postgres_openssl::MakeTlsConnector;
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::timeout;
use tracing::warn;

use crate::constants::SERVICE;
use crate::error::Error;
use crate::signals::labels::LabelEntry;

#[derive(Clone)]
pub struct ParametersDb {
    pool: Pool,
}

impl ParametersDb {
    const WAIT: Duration = Duration::from_secs(15);
    const CREATE: Duration = Duration::from_secs(10);
    const RECYCLE: Duration = Duration::from_secs(30);
    const DEALLOCATE_TIMEOUT: Duration = Duration::from_secs(5);
    const QUERY_TIMEOUT: Duration = Duration::from_secs(60);

    pub fn connect(database_url: impl Into<String>) -> Result<Self, Error> {
        let mut cfg = Config::new();
        cfg.url = Some(database_url.into());
        cfg.pool = Some(PoolConfig {
            max_size: 5,
            timeouts: deadpool_postgres::Timeouts {
                wait: Some(Self::WAIT),
                create: Some(Self::CREATE),
                recycle: Some(Self::RECYCLE),
            },
            ..Default::default()
        });

        let mut builder =
            SslConnector::builder(SslMethod::tls()).map_err(|e| Error::Internal(e.to_string()))?;
        builder.set_verify(openssl::ssl::SslVerifyMode::NONE);
        let tls = MakeTlsConnector::new(builder.build());
        let pool = cfg
            .create_pool(Some(Runtime::Tokio1), tls)
            .map_err(|e| Error::Internal(format!("db pool: {}", e)))?;
        Ok(Self { pool })
    }

    pub fn from_env_var(var_name: &str) -> Option<Result<Self, Error>> {
        let Ok(url) = std::env::var(var_name) else {
            return None;
        };
        if url.is_empty() {
            return None;
        }
        Some(Self::connect(url))
    }

    async fn conn(&self) -> Result<Client, Error> {
        self.pool
            .get()
            .await
            .map_err(|e| Error::Internal(format!("db pool: {}", e)))
    }

    /// Flat map for health check / legacy readers.
    pub async fn fetch_parameters_map(&self) -> Result<HashMap<String, String>, Error> {
        let rows = self.fetch_service_parameters(SERVICE).await?;
        let mut map = HashMap::new();
        for (endpoint, param_name, param_value) in &rows {
            map.insert(format!("{endpoint}:{param_name}"), param_value.clone());
            if endpoint == "*" {
                map.entry(param_name.clone())
                    .or_insert_with(|| param_value.clone());
            }
        }
        Ok(map)
    }

    pub async fn fetch_service_parameters(
        &self,
        service: &str,
    ) -> Result<Vec<(String, String, String)>, Error> {
        let mut client = self.conn().await?;
        let tx = client
            .transaction()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;

        let _ = timeout(Self::DEALLOCATE_TIMEOUT, tx.execute("DEALLOCATE ALL", &[])).await;

        let v2 = timeout(
            Self::QUERY_TIMEOUT,
            tx.query(
                r#"
                SELECT endpoint, param_name, param_value
                FROM parameters
                WHERE inactive = false
                  AND service = $1
                  AND (effective_from IS NULL OR effective_from <= NOW())
                  AND (expires_at IS NULL OR expires_at > NOW())
                ORDER BY endpoint ASC, param_name ASC
                "#,
                &[&service],
            ),
        )
        .await;

        match v2 {
            Ok(Ok(rows)) if !rows.is_empty() => {
                let out: Vec<(String, String, String)> = rows
                    .iter()
                    .map(|row| {
                        let endpoint: String = row.get("endpoint");
                        let param_name: String = row.get("param_name");
                        let param_value: String = row.get("param_value");
                        (endpoint, param_name, param_value)
                    })
                    .collect();
                tx.commit()
                    .await
                    .map_err(|e| Error::Internal(e.to_string()))?;
                return Ok(out);
            }
            Ok(Err(e)) => {
                warn!(error = %e, "v2 parameters query failed; trying legacy");
            }
            Err(_) => {
                return Err(Error::Internal("parameters query timed out".into()));
            }
            _ => {}
        }

        let rows = timeout(
            Self::QUERY_TIMEOUT,
            tx.query(
                r#"
                SELECT param_name, param_value
                FROM parameters
                WHERE inactive = false
                  AND (effective_from IS NULL OR effective_from <= NOW())
                  AND (expires_at IS NULL OR expires_at > NOW())
                ORDER BY param_name ASC
                "#,
                &[],
            ),
        )
        .await
        .map_err(|_| Error::Internal("parameters query timed out".into()))?
        .map_err(|e| Error::Internal(e.to_string()))?;

        let out: Vec<(String, String, String)> = rows
            .iter()
            .map(|row| {
                let param_name: String = row.get("param_name");
                let param_value: String = row.get("param_value");
                ("*".to_string(), param_name, param_value)
            })
            .collect();

        tx.commit()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(out)
    }

    pub async fn record_subscription(
        &self,
        payer: &str,
        tier: &str,
        issued_at: chrono::DateTime<chrono::Utc>,
        expires_at: chrono::DateTime<chrono::Utc>,
        tx_sig: Option<&str>,
    ) -> Result<(), Error> {
        let client = self.conn().await?;
        timeout(
            Self::QUERY_TIMEOUT,
            client.execute(
                r#"
                INSERT INTO solrisk_subscriptions (payer, tier, issued_at, expires_at, tx_sig)
                VALUES ($1, $2, $3, $4, $5)
                "#,
                &[&payer, &tier, &issued_at, &expires_at, &tx_sig],
            ),
        )
        .await
        .map_err(|_| Error::Internal("record_subscription timed out".into()))?
        .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(())
    }

    pub async fn is_revoked(&self, payer: &str, issued_at: i64) -> Result<bool, Error> {
        let client = self.conn().await?;
        let issued = chrono::DateTime::from_timestamp(issued_at, 0)
            .ok_or_else(|| Error::Internal("invalid iat".into()))?;
        let row = timeout(
            Self::QUERY_TIMEOUT,
            client.query_opt(
                r#"
                SELECT revoked FROM solrisk_subscriptions
                WHERE payer = $1 AND issued_at = $2
                LIMIT 1
                "#,
                &[&payer, &issued],
            ),
        )
        .await
        .map_err(|_| Error::Internal("is_revoked timed out".into()))?
        .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(row.map(|r| r.get::<_, bool>("revoked")).unwrap_or(false))
    }

    pub async fn check_and_increment_rate(
        &self,
        bucket_key: &str,
        limit: u32,
        _window_secs: u64,
    ) -> Result<bool, Error> {
        let client = self.conn().await?;
        let allowed = timeout(
            Self::QUERY_TIMEOUT,
            client.query_one(
                r#"
                WITH w AS (
                    SELECT date_trunc('minute', NOW()) AS window_start
                ),
                upsert AS (
                    INSERT INTO solrisk_rate_buckets (bucket_key, window_start, count)
                    SELECT $1, w.window_start, 1 FROM w
                    ON CONFLICT (bucket_key, window_start)
                    DO UPDATE SET count = solrisk_rate_buckets.count + 1
                    RETURNING count
                )
                SELECT count FROM upsert
                "#,
                &[&bucket_key],
            ),
        )
        .await
        .map_err(|_| Error::Internal("rate limit timed out".into()))?
        .map_err(|e| Error::Internal(e.to_string()))?;

        let count: i32 = allowed.get("count");
        Ok(count <= limit as i32)
    }

    pub async fn fetch_wallet_labels(&self) -> Result<Vec<LabelEntry>, Error> {
        let client = self.conn().await?;
        let rows = timeout(
            Self::QUERY_TIMEOUT,
            client.query(
                r#"
                SELECT wallet_pubkey, source, label, weight
                FROM solrisk_wallet_labels
                ORDER BY wallet_pubkey ASC
                "#,
                &[],
            ),
        )
        .await
        .map_err(|_| Error::Internal("labels query timed out".into()))?
        .map_err(|e| Error::Internal(e.to_string()))?;

        Ok(rows
            .iter()
            .map(|row| LabelEntry {
                wallet: row.get("wallet_pubkey"),
                source: row.get("source"),
                label: row.get("label"),
                weight: row.get("weight"),
                note: String::new(),
            })
            .collect())
    }

    pub async fn get_cached_score(
        &self,
        endpoint: &str,
        subject: &str,
    ) -> Result<Option<serde_json::Value>, Error> {
        let client = self.conn().await?;
        let row = timeout(
            Self::QUERY_TIMEOUT,
            client.query_opt(
                r#"
                SELECT response_json FROM solrisk_score_cache
                WHERE endpoint = $1 AND subject = $2 AND expires_at > NOW()
                "#,
                &[&endpoint, &subject],
            ),
        )
        .await
        .map_err(|_| Error::Internal("cache read timed out".into()))?
        .map_err(|e| Error::Internal(e.to_string()))?;

        Ok(row.map(|r| r.get::<_, serde_json::Value>("response_json")))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn set_cached_score(
        &self,
        endpoint: &str,
        subject: &str,
        score: i32,
        band: &str,
        response: &serde_json::Value,
        scoring_version: &str,
        ttl_secs: i64,
    ) -> Result<(), Error> {
        let client = self.conn().await?;
        timeout(
            Self::QUERY_TIMEOUT,
            client.execute(
                r#"
                INSERT INTO solrisk_score_cache
                    (endpoint, subject, score, band, response_json, scoring_version, cached_at, expires_at)
                VALUES ($1, $2, $3, $4, $5, $6, NOW(), NOW() + ($7 || ' seconds')::interval)
                ON CONFLICT (endpoint, subject) DO UPDATE SET
                    score = EXCLUDED.score,
                    band = EXCLUDED.band,
                    response_json = EXCLUDED.response_json,
                    scoring_version = EXCLUDED.scoring_version,
                    cached_at = NOW(),
                    expires_at = EXCLUDED.expires_at
                "#,
                &[
                    &endpoint,
                    &subject,
                    &score,
                    &band,
                    &response,
                    &scoring_version,
                    &ttl_secs.to_string(),
                ],
            ),
        )
        .await
        .map_err(|_| Error::Internal("cache write timed out".into()))?
        .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn log_scoring(
        &self,
        endpoint: &str,
        subject: &str,
        score: i32,
        band: &str,
        scoring_version: &str,
        signals_json: Option<serde_json::Value>,
        payer: Option<&str>,
        correlation_id: Option<&str>,
        settlement_sig: Option<&str>,
    ) -> Result<(), Error> {
        let client = self.conn().await?;
        let _ = timeout(
            Self::QUERY_TIMEOUT,
            client.execute(
                r#"
                INSERT INTO solrisk_scoring_log
                    (wallet_pubkey, endpoint, subject, score, band, scoring_version,
                     signals_json, payer, correlation_id, settlement_sig)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                "#,
                &[
                    &subject,
                    &endpoint,
                    &subject,
                    &score,
                    &band,
                    &scoring_version,
                    &signals_json,
                    &payer,
                    &correlation_id,
                    &settlement_sig,
                ],
            ),
        )
        .await;
        Ok(())
    }
}
