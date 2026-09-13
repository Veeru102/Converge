//! Converge server library: build the router and run it. `main.rs` reads
//! configuration from the environment; integration tests start the same
//! app in-process with in-memory storage.

#![forbid(unsafe_code)]

pub mod actor;
pub mod chaos;
pub mod config;
pub mod http;
pub mod registry;
pub mod ws;

use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;
use converge_storage::Storage;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use tower_http::cors::CorsLayer;

pub use chaos::{Chaos, ChaosConfig};
pub use config::Config;
pub use registry::Registry;

#[derive(Clone)]
pub struct AppState {
    pub cfg: Config,
    pub registry: Registry,
    pub chaos: Chaos,
    pub metrics: PrometheusHandle,
}

static METRICS: std::sync::OnceLock<PrometheusHandle> = std::sync::OnceLock::new();

fn metrics_handle() -> PrometheusHandle {
    METRICS
        .get_or_init(|| {
            PrometheusBuilder::new()
                .install_recorder()
                .expect("install prometheus recorder")
        })
        .clone()
}

pub fn app(cfg: Config, storage: Arc<dyn Storage>) -> (Router, AppState) {
    let state = AppState {
        registry: Registry::new(cfg.clone(), storage),
        chaos: Chaos::new(cfg.chaos_seed),
        metrics: metrics_handle(),
        cfg: cfg.clone(),
    };
    let mut router = Router::new()
        .route("/ws", get(ws::upgrade))
        .route("/healthz", get(http::healthz))
        .route("/metrics", get(http::metrics))
        .route("/docs", post(http::create_doc).get(http::list_docs))
        .route("/docs/:id/hash", get(http::doc_hash))
        .route("/docs/:id/snapshot", get(http::doc_snapshot))
        .route("/admin/chaos", get(http::get_chaos).put(http::put_chaos))
        .layer(CorsLayer::permissive())
        .with_state(state.clone());
    if let Some(dir) = &cfg.static_dir {
        let serve = tower_http::services::ServeDir::new(dir)
            .append_index_html_on_directories(true)
            .fallback(tower_http::services::ServeFile::new(format!(
                "{dir}/index.html"
            )));
        router = router.fallback_service(serve);
    }
    (router, state)
}

pub async fn serve(cfg: Config, storage: Arc<dyn Storage>) -> anyhow::Result<()> {
    let (router, state) = app(cfg.clone(), storage);
    let listener = tokio::net::TcpListener::bind(&cfg.bind).await?;
    tracing::info!(bind = %cfg.bind, "listening");
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutting down");
            state.registry.shutdown().await;
        })
        .await?;
    Ok(())
}
