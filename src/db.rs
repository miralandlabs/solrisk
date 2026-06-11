//! Optional Postgres: parameters (v2 service/endpoint), subscriptions, rate limits, cache, audit.
//!
//! All SQL runs inside an explicit transaction with `SET LOCAL statement_timeout`,
//! best-effort `DEALLOCATE ALL`, and wall-clock `timeout()` on pool get, BEGIN,
//! each statement, and COMMIT — same pattern as `x402-buy-spl-token/src/db.rs`.

use deadpool_postgres::{Client, Config, Pool, PoolConfig, Runtime};
use openssl::ssl::{SslConnector, SslMethod};
use postgres_openssl::MakeTlsConnector;
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::timeout;
use tokio_postgres::types::ToSql;
use tracing::{error, warn};

use crate::constants::SERVICE;
use crate::error::Error;
use crate::signals::labels::LabelEntry;

#[derive(Clone)]
pub struct ParametersDb {
    pool: Pool,
}

impl ParametersDb {
    // --- Pool timeouts (deadpool-level) ---
    const WAIT: Duration = Duration::from_secs(15);
    const CREATE: Duration = Duration::from_secs(10);
    const RECYCLE: Duration = Duration::from_secs(30);

    // --- Per-call timeouts (tokio-level, wrap every wire step) ---
    const POOL_GET_TIMEOUT: Duration = Duration::from_secs(20);
    const TX_BEGIN_TIMEOUT: Duration = Duration::from_secs(20);
    const SET_LOCAL_CMD_TIMEOUT: Duration = Duration::from_secs(5);
    const QUERY_TIMEOUT: Duration = Duration::from_secs(60);

    /// Per-statement ceiling enforced by Postgres (below [`Self::QUERY_TIMEOUT`]).
    const PG_STATEMENT_TIMEOUT: &'static str = "25s";

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
        timeout(Self::POOL_GET_TIMEOUT, self.pool.get())
            .await
            .map_err(|_| {
                Error::Internal(format!(
                    "db pool get timed out after {:?}",
                    Self::POOL_GET_TIMEOUT
                ))
            })?
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
        let label = "fetch service parameters";
        let tx = Self::open_transaction(&mut client, label).await?;

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
                Self::commit_transaction(tx, label).await?;
                return Ok(out);
            }
            Ok(Err(e)) => {
                warn!(error = %e, "v2 parameters query failed; trying legacy");
            }
            Err(_) => {
                return Err(Error::Internal(format!(
                    "{} timed out after {:?}",
                    label,
                    Self::QUERY_TIMEOUT
                )));
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
        .map_err(|_| {
            Error::Internal(format!(
                "{} legacy timed out after {:?}",
                label,
                Self::QUERY_TIMEOUT
            ))
        })?
        .map_err(|e| Error::Internal(format!("{} legacy query failed: {}", label, e)))?;

        let out: Vec<(String, String, String)> = rows
            .iter()
            .map(|row| {
                let param_name: String = row.get("param_name");
                let param_value: String = row.get("param_value");
                ("*".to_string(), param_name, param_value)
            })
            .collect();

        Self::commit_transaction(tx, label).await?;
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
        self.exec_in_tx(
            client,
            r#"
            INSERT INTO solrisk_subscriptions (payer, tier, issued_at, expires_at, tx_sig)
            VALUES ($1, $2, $3, $4, $5)
            "#,
            &[&payer, &tier, &issued_at, &expires_at, &tx_sig],
            "record subscription",
        )
        .await?;
        Ok(())
    }

    pub async fn is_revoked(&self, payer: &str, issued_at: i64) -> Result<bool, Error> {
        let issued = chrono::DateTime::from_timestamp(issued_at, 0)
            .ok_or_else(|| Error::Internal("invalid iat".into()))?;
        let client = self.conn().await?;
        let row = self
            .query_opt_in_tx(
                client,
                r#"
                SELECT revoked FROM solrisk_subscriptions
                WHERE payer = $1 AND issued_at = $2
                LIMIT 1
                "#,
                &[&payer, &issued],
                "is revoked",
            )
            .await?;
        Ok(row.map(|r| r.get::<_, bool>("revoked")).unwrap_or(false))
    }

    pub async fn check_and_increment_rate(
        &self,
        bucket_key: &str,
        limit: u32,
        _window_secs: u64,
    ) -> Result<bool, Error> {
        let client = self.conn().await?;
        let rows = self
            .query_in_tx(
                client,
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
                "rate limit",
            )
            .await?;

        let count: i32 = rows.first().map(|r| r.get("count")).unwrap_or(0);
        Ok(count as i64 <= limit as i64)
    }

    pub async fn fetch_wallet_labels(&self) -> Result<Vec<LabelEntry>, Error> {
        let client = self.conn().await?;
        let rows = self
            .query_in_tx(
                client,
                r#"
                SELECT wallet_pubkey, source, label, weight
                FROM solrisk_wallet_labels
                ORDER BY wallet_pubkey ASC
                "#,
                &[],
                "fetch wallet labels",
            )
            .await?;

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
    ) -> Result<Option<(serde_json::Value, chrono::DateTime<chrono::Utc>)>, Error> {
        let client = self.conn().await?;
        let row = self
            .query_opt_in_tx(
                client,
                r#"
                SELECT response_json, cached_at FROM solrisk_score_cache
                WHERE endpoint = $1 AND subject = $2 AND expires_at > NOW()
                "#,
                &[&endpoint, &subject],
                "cache read",
            )
            .await?;

        Ok(row.map(|r| {
            let json: serde_json::Value = r.get("response_json");
            let cached_at: chrono::DateTime<chrono::Utc> = r.get("cached_at");
            (json, cached_at)
        }))
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
        let mut client = self.conn().await?;
        let label = "cache write";
        let tx = Self::open_transaction(&mut client, label).await?;

        // DELETE + INSERT instead of ON CONFLICT — shared Supabase DBs may still
        // have a legacy PK from v0.1 where 002_parameters_v2 PK migration failed
        // silently (EXCEPTION handler), which breaks ON CONFLICT (endpoint, subject).
        timeout(
            Self::QUERY_TIMEOUT,
            tx.execute(
                "DELETE FROM solrisk_score_cache WHERE endpoint = $1 AND subject = $2",
                &[&endpoint, &subject],
            ),
        )
        .await
        .map_err(|_| {
            Error::Internal(format!(
                "{} delete timed out after {:?}",
                label,
                Self::QUERY_TIMEOUT
            ))
        })?
        .map_err(|e| Error::Internal(format!("{} delete failed: {}", label, e)))?;

        let ttl: i32 = ttl_secs.clamp(1, 86400) as i32;
        let rows = timeout(
            Self::QUERY_TIMEOUT,
            tx.execute(
                r#"
                INSERT INTO solrisk_score_cache
                    (endpoint, subject, score, band, response_json, scoring_version, cached_at, expires_at)
                VALUES ($1, $2, $3, $4, $5, $6, NOW(), NOW() + ($7::int * INTERVAL '1 second'))
                "#,
                &[
                    &endpoint,
                    &subject,
                    &score,
                    &band,
                    &response,
                    &scoring_version,
                    &ttl,
                ],
            ),
        )
        .await
        .map_err(|_| {
            Error::Internal(format!(
                "{} insert timed out after {:?}",
                label,
                Self::QUERY_TIMEOUT
            ))
        })?
        .map_err(|e| Error::Internal(format!("{} insert failed: {}", label, e)))?;

        Self::commit_transaction(tx, label).await?;
        tracing::debug!(endpoint, subject, rows, ttl_secs, "score cache written");
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
        match self
            .exec_in_tx(
                client,
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
                "scoring log",
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(e) => {
                warn!(error = %e, "scoring log insert failed");
                Ok(())
            }
        }
    }

    // --- Transaction helpers (mirror x402-buy-spl-token) --------------------

    async fn open_transaction<'a>(
        client: &'a mut Client,
        label: &str,
    ) -> Result<deadpool_postgres::Transaction<'a>, Error> {
        let tx = Self::begin_transaction(client, label).await?;
        Self::set_statement_timeout_local(&tx).await;
        Ok(tx)
    }

    async fn begin_transaction<'a>(
        client: &'a mut Client,
        label: &str,
    ) -> Result<deadpool_postgres::Transaction<'a>, Error> {
        timeout(Self::TX_BEGIN_TIMEOUT, client.transaction())
            .await
            .map_err(|_| {
                Error::Internal(format!(
                    "{} transaction start timed out after {:?} (pool connection may be stale)",
                    label,
                    Self::TX_BEGIN_TIMEOUT
                ))
            })?
            .map_err(|e| Error::Internal(format!("{} transaction start failed: {}", label, e)))
    }

    async fn set_statement_timeout_local(tx: &deadpool_postgres::Transaction<'_>) {
        let sql = format!(
            "SET LOCAL statement_timeout = '{}'",
            Self::PG_STATEMENT_TIMEOUT
        );
        match timeout(Self::SET_LOCAL_CMD_TIMEOUT, tx.execute(sql.as_str(), &[])).await {
            Ok(Ok(_)) => (),
            Ok(Err(e)) => error!(error = %e, "SET LOCAL statement_timeout failed"),
            Err(_) => error!(
                "SET LOCAL statement_timeout timed out after {:?}",
                Self::SET_LOCAL_CMD_TIMEOUT
            ),
        }
    }



    async fn commit_transaction(
        tx: deadpool_postgres::Transaction<'_>,
        label: &str,
    ) -> Result<(), Error> {
        timeout(Self::QUERY_TIMEOUT, tx.commit())
            .await
            .map_err(|_| {
                Error::Internal(format!(
                    "{} commit timed out after {:?}",
                    label,
                    Self::QUERY_TIMEOUT
                ))
            })?
            .map_err(|e| Error::Internal(format!("{} commit failed: {}", label, e)))
    }

    async fn exec_in_tx(
        &self,
        mut client: Client,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
        label: &str,
    ) -> Result<u64, Error> {
        let tx = Self::open_transaction(&mut client, label).await?;

        let rows = timeout(Self::QUERY_TIMEOUT, tx.execute(sql, params))
            .await
            .map_err(|_| {
                Error::Internal(format!(
                    "{} timed out after {:?}",
                    label,
                    Self::QUERY_TIMEOUT
                ))
            })?
            .map_err(|e| Error::Internal(format!("{} query failed: {}", label, e)))?;

        Self::commit_transaction(tx, label).await?;
        Ok(rows)
    }

    async fn query_opt_in_tx(
        &self,
        mut client: Client,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
        label: &str,
    ) -> Result<Option<tokio_postgres::Row>, Error> {
        let tx = Self::open_transaction(&mut client, label).await?;

        let row = timeout(Self::QUERY_TIMEOUT, tx.query_opt(sql, params))
            .await
            .map_err(|_| {
                Error::Internal(format!(
                    "{} timed out after {:?}",
                    label,
                    Self::QUERY_TIMEOUT
                ))
            })?
            .map_err(|e| Error::Internal(format!("{} query failed: {}", label, e)))?;

        Self::commit_transaction(tx, label).await?;
        Ok(row)
    }

    async fn query_in_tx(
        &self,
        mut client: Client,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
        label: &str,
    ) -> Result<Vec<tokio_postgres::Row>, Error> {
        let tx = Self::open_transaction(&mut client, label).await?;

        let rows = timeout(Self::QUERY_TIMEOUT, tx.query(sql, params))
            .await
            .map_err(|_| {
                Error::Internal(format!(
                    "{} timed out after {:?}",
                    label,
                    Self::QUERY_TIMEOUT
                ))
            })?
            .map_err(|e| Error::Internal(format!("{} query failed: {}", label, e)))?;

        Self::commit_transaction(tx, label).await?;
        Ok(rows)
    }
}
