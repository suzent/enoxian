use axum::{extract::{Path, State}, http::StatusCode, response::IntoResponse, Json};
use serde_json::json;
use crate::daemon::DaemonState;

pub async fn list_files(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => return (StatusCode::NOT_FOUND, Json(json!({"error": "circle not found"}))).into_response(),
    };

    // state.docs is the authoritative set of tracked files — populated by startup preload
    // and updated as files are synced from peers or created locally. No filesystem scan needed.
    let mut files: Vec<String> = state.docs
        .iter()
        .map(|r| r.key().clone())
        .filter(|k| k != "__control__")
        .collect();

    files.sort();
    Json(json!(files)).into_response()
}
