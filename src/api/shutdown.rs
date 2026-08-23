use crate::daemon::DaemonState;
use axum::{extract::State, response::IntoResponse, Json};
use serde_json::json;

pub async fn shutdown(State(daemon): State<DaemonState>) -> impl IntoResponse {
    tracing::info!("shutdown requested via API");
    daemon.shutdown();
    Json(json!({ "status": "stopping" }))
}
