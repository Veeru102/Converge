use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use converge_proto::DocId;
use tokio::sync::oneshot;

use crate::actor::DocCommand;
use crate::chaos::ChaosConfig;
use crate::AppState;

pub async fn healthz() -> &'static str {
    "ok"
}

pub async fn metrics(State(state): State<AppState>) -> String {
    state.metrics.render()
}

pub async fn create_doc(State(state): State<AppState>) -> impl IntoResponse {
    let id = DocId(uuid::Uuid::now_v7().to_string());
    match state.registry.storage().ensure_doc(&id).await {
        Ok(()) => (StatusCode::CREATED, Json(serde_json::json!({ "id": id.0 }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn list_docs(State(state): State<AppState>) -> impl IntoResponse {
    match state.registry.storage().list_docs().await {
        Ok(ids) => Json(serde_json::json!({ "docs": ids.iter().map(|d| d.0.clone()).collect::<Vec<_>>(), "open": state.registry.open_docs().iter().map(|d| d.0.clone()).collect::<Vec<_>>() })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Durable state hash — the convergence oracle used by the e2e tests.
pub async fn doc_hash(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    let handle = match state.registry.get_or_spawn(&DocId(id)).await {
        Ok(h) => h,
        Err(e) => return (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    };
    let (reply, rx) = oneshot::channel();
    if handle.tx.send(DocCommand::Inspect { reply }).await.is_err() {
        return (StatusCode::SERVICE_UNAVAILABLE, "document unloading").into_response();
    }
    match rx.await {
        Ok(i) => Json(i).into_response(),
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "document unloading").into_response(),
    }
}

pub async fn doc_snapshot(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let handle = match state.registry.get_or_spawn(&DocId(id)).await {
        Ok(h) => h,
        Err(e) => return (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    };
    let (reply, rx) = oneshot::channel();
    let _ = handle.tx.send(DocCommand::Snapshot { reply }).await;
    match rx.await {
        Ok((bytes, seq)) => (
            [
                ("content-type", "application/octet-stream"),
                ("x-converge-seq", &seq.to_string()),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "document unloading").into_response(),
    }
}

pub async fn get_chaos(State(state): State<AppState>) -> Json<ChaosConfig> {
    Json(state.chaos.get())
}

pub async fn put_chaos(
    State(state): State<AppState>,
    Json(cfg): Json<ChaosConfig>,
) -> Json<ChaosConfig> {
    tracing::info!(?cfg, "chaos configuration updated");
    state.chaos.set(cfg.clone());
    Json(cfg)
}
