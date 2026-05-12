use {
    crate::{config::Config, db::ParametersDb, error::Error, x402::FacilitatorClient},
    solana_commitment_config::CommitmentConfig,
    std::sync::Arc,
};

/// Application state (one facilitator client + optional parameters DB per cold start).
pub struct AppState {
    pub config: Arc<Config>,
    pub rpc_client: Arc<solana_client::nonblocking::rpc_client::RpcClient>,
    pub db: Option<Arc<ParametersDb>>,
    pub facilitator: Arc<FacilitatorClient>,
}

impl AppState {
    pub fn new(config: &Config) -> Result<Self, Error> {
        // `Confirmed` is the right commitment for a balance read: it reflects
        // the latest block that the cluster has agreed on (sub-second) without
        // waiting for `Finalized` (~13 s). Freshly-created ATAs become visible
        // immediately — important because the buyer just signed & landed a tx
        // moments ago when they reach this endpoint.
        let rpc_client = Arc::new(
            solana_client::nonblocking::rpc_client::RpcClient::new_with_commitment(
                config.solana_rpc_url.clone(),
                CommitmentConfig::confirmed(),
            ),
        );

        let db = match ParametersDb::from_env_var("DATABASE_URL") {
            None => None,
            Some(Ok(d)) => Some(Arc::new(d)),
            Some(Err(e)) => {
                tracing::warn!(error = %e, "DATABASE_URL set but connection failed; using env only");
                None
            }
        };

        let facilitator = FacilitatorClient::new(config.x402_facilitator_url.clone());

        Ok(Self {
            config: Arc::new(config.clone()),
            rpc_client,
            db,
            facilitator: Arc::new(facilitator),
        })
    }
}

impl Clone for AppState {
    fn clone(&self) -> Self {
        Self {
            config: Arc::clone(&self.config),
            rpc_client: Arc::clone(&self.rpc_client),
            db: self.db.as_ref().map(Arc::clone),
            facilitator: Arc::clone(&self.facilitator),
        }
    }
}
