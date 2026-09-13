use std::sync::Arc;

use converge_server::Config;
use converge_storage::{MemoryStorage, PgStorage, Storage};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sqlx=warn".into()),
        )
        .init();
    let cfg = Config::from_env();
    let storage: Arc<dyn Storage> = match &cfg.database_url {
        Some(url) => {
            tracing::info!("using postgres storage");
            Arc::new(PgStorage::connect(url).await?)
        }
        None => {
            tracing::warn!(
                "DATABASE_URL not set: using in-memory storage (nothing survives a restart)"
            );
            Arc::new(MemoryStorage::new())
        }
    };
    converge_server::serve(cfg, storage).await
}
