use axum::{extract::{Path, State}, http::StatusCode, response::IntoResponse, Json};
use serde_json::json;
use crate::{config, daemon::DaemonState, lifecycle};

pub async fn stop_circle(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
) -> impl IntoResponse {
    if daemon.stop_circle(&circle_id) {
        tracing::info!("[lifecycle] stopped circle {circle_id}");
        Json(json!({"status": "stopped", "circle_id": circle_id})).into_response()
    } else {
        (StatusCode::NOT_FOUND, Json(json!({"error": "circle not active"}))).into_response()
    }
}

pub async fn start_circle(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
) -> impl IntoResponse {
    if daemon.is_active(&circle_id) {
        return (StatusCode::CONFLICT, Json(json!({"error": "circle already running"}))).into_response();
    }

    let cfg = match config::load(&circle_id) {
        Ok(c) => c,
        Err(e) => return (StatusCode::NOT_FOUND, Json(json!({"error": e.to_string()}))).into_response(),
    };

    if cfg.disabled {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "circle is disabled — run `enox enable` first"}))).into_response();
    }

    match lifecycle::spawn_circle(cfg, daemon).await {
        Ok(()) => {
            tracing::info!("[lifecycle] started circle {circle_id}");
            Json(json!({"status": "started", "circle_id": circle_id})).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}
