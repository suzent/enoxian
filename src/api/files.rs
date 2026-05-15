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

    let mut files: Vec<String> = Vec::new();
    let mut stack = vec![state.workspace.clone()];

    while let Some(dir) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(e) => e,
            Err(_) => continue,
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let rel = path.strip_prefix(&state.workspace).unwrap_or(&path);
            let rel_str = rel.to_string_lossy().replace('\\', "/");

            // Skip hidden files, temp files, and CRDT state directory
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if name.starts_with('.') || name.starts_with("__") || name.ends_with(".swp") {
                continue;
            }

            if path.is_dir() {
                stack.push(path);
            } else {
                files.push(rel_str);
            }
        }
    }

    files.sort();
    Json(json!(files)).into_response()
}
