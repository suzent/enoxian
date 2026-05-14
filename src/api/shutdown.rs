use axum::{extract::State, response::IntoResponse, Json};
use serde_json::json;
use crate::daemon::DaemonState;

pub async fn shutdown(State(daemon): State<DaemonState>) -> impl IntoResponse {
    tracing::info!("shutdown requested via API");
    daemon.shutdown_token.cancel();
    Json(json!({ "status": "stopping" }))
}
