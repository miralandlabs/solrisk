//! Optional Postgres: parameters (v2 service/endpoint), subscriptions, rate limits, cache, audit.
//!
//! All SQL runs inside an explicit transaction with `SET LOCAL statement_timeout`,
//! best-effort `DEALLOCATE ALL`, and wall-clock `timeout()` on pool get, BEGIN,
//! each statement, and COMMIT — same pattern as `x402-buy-spl-token/src/db.rs`.

use deadpool_postgres::{
    Client, Config, ManagerConfig, Pool, PoolConfig, RecyclingMethod, Runtime,
};
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
    const DEALLOCATE_TIMEOUT: Duration = Duration::from_secs(5);

    /// Per-statement ceiling enforced by Postgres (below [`Self::QUERY_TIMEOUT`]).
    const PG_STATEMENT_TIMEOUT: &'static str = "25s";

    pub fn connect(database_url: impl Into<String>) -> Result<Self, Error> {
        let mut cfg = Config::new();
        cfg.url = Some(database_url.into());
        cfg.manager = Some(ManagerConfig {
            recycling_method: RecyclingMethod::Clean,
        });
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

    /// Lightweight liveness probe for `/health` (no transaction wrapper).
    pub async fn ping(&self) -> Result<(), Error> {
        const PING_TIMEOUT: Duration = Duration::from_secs(8);
        let client = self.conn().await?;
        let label = "health ping";
        match timeout(PING_TIMEOUT, client.simple_query("SELECT 1")).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => {
                Self::discard_client(client, label, "ping failed");
                Err(Error::Internal(format!("{} failed: {}", label, e)))
            }
            Err(_) => {
                Self::discard_client(client, label, "ping timed out");
                Err(Error::Internal(format!(
                    "{} timed out after {:?}",
                    label, PING_TIMEOUT
                )))
            }
        }
    }

    /// Flat map for legacy readers.
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
        let v2_rows = self
            .query_in_tx(
                self.conn().await?,
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
                "fetch service parameters",
            )
            .await;

        match v2_rows {
            Ok(rows) if !rows.is_empty() => {
                return Ok(rows
                    .iter()
                    .map(|row| {
                        (
                            row.get("endpoint"),
                            row.get("param_name"),
                            row.get("param_value"),
                        )
                    })
                    .collect());
            }
            Ok(_) => {}
            Err(e) => warn!(error = %e, "v2 parameters query failed; trying legacy"),
        }

        let rows = self
            .query_in_tx(
                self.conn().await?,
                r#"
                SELECT param_name, param_value
                FROM parameters
                WHERE inactive = false
                  AND (effective_from IS NULL OR effective_from <= NOW())
                  AND (expires_at IS NULL OR expires_at > NOW())
                ORDER BY param_name ASC
                "#,
                &[],
                "fetch service parameters legacy",
            )
            .await?;

        let out: Vec<(String, String, String)> = rows
            .iter()
            .map(|row| {
                let param_name: String = row.get("param_name");
                let param_value: String = row.get("param_value");
                ("*".to_string(), param_name, param_value)
            })
            .collect();

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
        let client = self.conn().await?;
        let label = "cache write";
        match Self::open_transaction(&client, label).await {
            Ok(()) => {}
            Err(e) => {
                Self::discard_client(client, label, "open transaction failed");
                return Err(e);
            }
        }

        // DELETE + INSERT instead of ON CONFLICT — shared Supabase DBs may still
        // have a legacy PK from v0.1 where 002_parameters_v2 PK migration failed
        // silently (EXCEPTION handler), which breaks ON CONFLICT (endpoint, subject).
        match timeout(
            Self::QUERY_TIMEOUT,
            client.execute(
                "DELETE FROM solrisk_score_cache WHERE endpoint = $1 AND subject = $2",
                &[&endpoint, &subject],
            ),
        )
        .await
        {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                Self::discard_client(client, label, "delete failed");
                return Err(Error::Internal(format!("{} delete failed: {}", label, e)));
            }
            Err(_) => {
                Self::discard_client(client, label, "delete timed out");
                return Err(Error::Internal(format!(
                    "{} delete timed out after {:?}",
                    label,
                    Self::QUERY_TIMEOUT
                )));
            }
        }

        let ttl: i32 = ttl_secs.clamp(1, 86400) as i32;
        let rows = match timeout(
            Self::QUERY_TIMEOUT,
            client.execute(
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
        {
            Ok(Ok(rows)) => rows,
            Ok(Err(e)) => {
                Self::discard_client(client, label, "insert failed");
                return Err(Error::Internal(format!("{} insert failed: {}", label, e)));
            }
            Err(_) => {
                Self::discard_client(client, label, "insert timed out");
                return Err(Error::Internal(format!(
                    "{} insert timed out after {:?}",
                    label,
                    Self::QUERY_TIMEOUT
                )));
            }
        };

        if let Err(e) = Self::commit_transaction(&client, label).await {
            Self::discard_client(client, label, "commit failed");
            return Err(e);
        }
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
                Err(e)
            }
        }
    }

    // --- Transaction helpers (mirror x402-buy-spl-token) --------------------

    async fn begin_transaction(client: &Client, label: &str) -> Result<(), Error> {
        timeout(Self::TX_BEGIN_TIMEOUT, client.batch_execute("BEGIN"))
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

    async fn open_transaction(client: &Client, label: &str) -> Result<(), Error> {
        Self::begin_transaction(client, label).await?;
        Self::set_statement_timeout_local(client).await?;
        Self::deallocate_prepared(client).await?;
        Ok(())
    }

    async fn set_statement_timeout_local(client: &Client) -> Result<(), Error> {
        let sql = format!(
            "SET LOCAL statement_timeout = '{}'",
            Self::PG_STATEMENT_TIMEOUT
        );
        match timeout(
            Self::SET_LOCAL_CMD_TIMEOUT,
            client.batch_execute(sql.as_str()),
        )
        .await
        {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => {
                error!(error = %e, "SET LOCAL statement_timeout failed");
                Err(Error::Internal(format!(
                    "SET LOCAL statement_timeout failed: {}",
                    e
                )))
            }
            Err(_) => {
                error!(
                    "SET LOCAL statement_timeout timed out after {:?}",
                    Self::SET_LOCAL_CMD_TIMEOUT
                );
                Err(Error::Internal(format!(
                    "SET LOCAL statement_timeout timed out after {:?}",
                    Self::SET_LOCAL_CMD_TIMEOUT
                )))
            }
        }
    }

    async fn deallocate_prepared(client: &Client) -> Result<(), Error> {
        match timeout(
            Self::DEALLOCATE_TIMEOUT,
            client.batch_execute("DEALLOCATE ALL"),
        )
        .await
        {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => Err(Error::Internal(format!("DEALLOCATE ALL failed: {}", e))),
            Err(_) => Err(Error::Internal(format!(
                "DEALLOCATE ALL timed out after {:?}",
                Self::DEALLOCATE_TIMEOUT
            ))),
        }
    }

    fn discard_client(client: Client, label: &str, reason: &str) {
        warn!(label, reason, "discarding db pool client");
        drop(Client::take(client));
    }

    async fn commit_transaction(client: &Client, label: &str) -> Result<(), Error> {
        timeout(Self::QUERY_TIMEOUT, client.batch_execute("COMMIT"))
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
        client: Client,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
        label: &str,
    ) -> Result<u64, Error> {
        match Self::open_transaction(&client, label).await {
            Ok(()) => {}
            Err(e) => {
                Self::discard_client(client, label, "open transaction failed");
                return Err(e);
            }
        }

        let rows = match timeout(Self::QUERY_TIMEOUT, client.execute(sql, params)).await {
            Ok(Ok(rows)) => rows,
            Ok(Err(e)) => {
                Self::discard_client(client, label, "query failed");
                return Err(Error::Internal(format!("{} query failed: {}", label, e)));
            }
            Err(_) => {
                Self::discard_client(client, label, "query timed out");
                return Err(Error::Internal(format!(
                    "{} timed out after {:?}",
                    label,
                    Self::QUERY_TIMEOUT
                )));
            }
        };

        if let Err(e) = Self::commit_transaction(&client, label).await {
            Self::discard_client(client, label, "commit failed");
            return Err(e);
        }
        Ok(rows)
    }

    async fn query_opt_in_tx(
        &self,
        client: Client,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
        label: &str,
    ) -> Result<Option<tokio_postgres::Row>, Error> {
        match Self::open_transaction(&client, label).await {
            Ok(()) => {}
            Err(e) => {
                Self::discard_client(client, label, "open transaction failed");
                return Err(e);
            }
        }

        let row = match timeout(Self::QUERY_TIMEOUT, client.query_opt(sql, params)).await {
            Ok(Ok(row)) => row,
            Ok(Err(e)) => {
                Self::discard_client(client, label, "query failed");
                return Err(Error::Internal(format!("{} query failed: {}", label, e)));
            }
            Err(_) => {
                Self::discard_client(client, label, "query timed out");
                return Err(Error::Internal(format!(
                    "{} timed out after {:?}",
                    label,
                    Self::QUERY_TIMEOUT
                )));
            }
        };

        if let Err(e) = Self::commit_transaction(&client, label).await {
            Self::discard_client(client, label, "commit failed");
            return Err(e);
        }
        Ok(row)
    }

    async fn query_in_tx(
        &self,
        client: Client,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
        label: &str,
    ) -> Result<Vec<tokio_postgres::Row>, Error> {
        match Self::open_transaction(&client, label).await {
            Ok(()) => {}
            Err(e) => {
                Self::discard_client(client, label, "open transaction failed");
                return Err(e);
            }
        }

        let rows = match timeout(Self::QUERY_TIMEOUT, client.query(sql, params)).await {
            Ok(Ok(rows)) => rows,
            Ok(Err(e)) => {
                Self::discard_client(client, label, "query failed");
                return Err(Error::Internal(format!("{} query failed: {}", label, e)));
            }
            Err(_) => {
                Self::discard_client(client, label, "query timed out");
                return Err(Error::Internal(format!(
                    "{} timed out after {:?}",
                    label,
                    Self::QUERY_TIMEOUT
                )));
            }
        };

        if let Err(e) = Self::commit_transaction(&client, label).await {
            Self::discard_client(client, label, "commit failed");
            return Err(e);
        }
        Ok(rows)
    }
}
