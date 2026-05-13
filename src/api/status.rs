use axum::{extract::{Path, State}, http::StatusCode, response::IntoResponse, Json};
use serde_json::json;
use crate::daemon::DaemonState;

pub async fn get_status(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => return (StatusCode::NOT_FOUND, Json(json!({"error": "circle not found"}))).into_response(),
    };
    Json(json!({
        "circle_id":   state.circle_id,
        "circle_name": state.circle_name,
        "workspace":   state.sync_dir.to_string_lossy(),
        "docs":        state.docs.len(),
    })).into_response()
}
