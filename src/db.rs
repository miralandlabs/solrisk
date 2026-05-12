//! Optional Postgres `parameters` store (pr402-compatible table shape).

use deadpool_postgres::{Client, Config, Pool, PoolConfig, Runtime};
use openssl::ssl::{SslConnector, SslMethod};
use postgres_openssl::MakeTlsConnector;
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::timeout;

use crate::error::Error;

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

    /// `None` if unset; `Some(Err)` if URL unusable.
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

    pub async fn fetch_parameters_map(&self) -> Result<HashMap<String, String>, Error> {
        let mut client = self.conn().await?;
        let tx = client
            .transaction()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;

        let _ = timeout(Self::DEALLOCATE_TIMEOUT, tx.execute("DEALLOCATE ALL", &[])).await;

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

        let map: HashMap<String, String> = rows
            .iter()
            .map(|row| {
                let name: String = row.get("param_name");
                let value: String = row.get("param_value");
                (name, value)
            })
            .collect();

        tx.commit()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(map)
    }
}
