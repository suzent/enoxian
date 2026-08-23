use crate::control::CircleEvent;
use crate::daemon::DaemonState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;
use std::path::{Component, PathBuf};
use yrs::{Text, Transact, WriteTxn};

pub async fn list_files(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "circle not found"})),
            )
                .into_response();
        }
    };

    // state.docs is the authoritative set of tracked files — populated by startup preload
    // and updated as files are synced from peers or created locally. No filesystem scan needed.
    let mut files: Vec<String> = state
        .docs
        .iter()
        .map(|r| r.key().clone())
        .filter(|k| k != "__control__")
        .collect();

    files.sort();
    Json(json!(files)).into_response()
}

#[derive(Deserialize)]
pub struct CreateFileRequest {
    pub path: String,
    pub content: Option<String>,
}

#[derive(Deserialize)]
pub struct RenameFileRequest {
    pub from: String,
    pub to: String,
}

#[derive(Deserialize)]
pub struct DeleteFileRequest {
    pub path: String,
}

fn normalize_rel_path(raw: &str) -> Result<String, &'static str> {
    let trimmed = raw.trim().replace('\\', "/");
    if trimmed.is_empty() {
        return Err("path is required");
    }

    let path = PathBuf::from(&trimmed);
    if path.is_absolute() {
        return Err("absolute paths are not allowed");
    }

    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_string_lossy();
                if part.is_empty() || part == "." || part == ".." {
                    return Err("invalid path segment");
                }
                parts.push(part.to_string());
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("path must stay inside the workspace");
            }
        }
    }

    if parts.is_empty() {
        return Err("path is required");
    }
    Ok(parts.join("/"))
}

fn workspace_path(state: &crate::state::AppState, rel: &str) -> PathBuf {
    state
        .workspace
        .join(rel.replace('/', std::path::MAIN_SEPARATOR_STR))
}

pub async fn create_file(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Json(req): Json<CreateFileRequest>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "circle not found"})),
            )
                .into_response();
        }
    };
    let rel = match normalize_rel_path(&req.path) {
        Ok(path) => path,
        Err(error) => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
        }
    };
    let full = workspace_path(&state, &rel);
    if tokio::fs::try_exists(&full).await.unwrap_or(false) {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "file already exists" })),
        )
            .into_response();
    }
    if let Some(parent) = full.parent() {
        if let Err(err) = tokio::fs::create_dir_all(parent).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": err.to_string() })),
            )
                .into_response();
        }
    }

    let content = req.content.unwrap_or_default();
    if let Err(err) = tokio::fs::write(&full, &content).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        )
            .into_response();
    }

    let doc = state.get_or_create_doc(&rel);
    let mut txn = match doc.try_transact_mut() {
        Ok(txn) => txn,
        Err(_) => return super::circle_busy(),
    };
    let text = txn.get_or_insert_text(rel.as_str());
    if !content.is_empty() {
        text.insert(&mut txn, 0, &content);
    }
    let _ = state.interactive_writes.send((rel.clone(), None));
    let _ = state
        .events
        .send(CircleEvent::FileUpdated { path: rel.clone() });

    (
        StatusCode::OK,
        Json(json!({ "status": "created", "path": rel })),
    )
        .into_response()
}

pub async fn rename_file(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Json(req): Json<RenameFileRequest>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "circle not found"})),
            )
                .into_response();
        }
    };
    let from = match normalize_rel_path(&req.from) {
        Ok(path) => path,
        Err(error) => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
        }
    };
    let to = match normalize_rel_path(&req.to) {
        Ok(path) => path,
        Err(error) => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
        }
    };
    let from_full = workspace_path(&state, &from);
    let to_full = workspace_path(&state, &to);
    if !tokio::fs::try_exists(&from_full).await.unwrap_or(false) {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "source file not found" })),
        )
            .into_response();
    }
    if tokio::fs::try_exists(&to_full).await.unwrap_or(false) {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "destination already exists" })),
        )
            .into_response();
    }
    if let Some(parent) = to_full.parent() {
        if let Err(err) = tokio::fs::create_dir_all(parent).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": err.to_string() })),
            )
                .into_response();
        }
    }

    if let Err(err) = tokio::fs::rename(&from_full, &to_full).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        )
            .into_response();
    }

    state.remove_doc(&from);
    crate::store::crdt::delete(&state.workspace, &from).await;
    let _ = state.all_deletes.send(from.clone());
    let _ = state.interactive_writes.send((from.clone(), None));
    let _ = state
        .events
        .send(CircleEvent::FileDeleted { path: from.clone() });

    if let Ok(content) = tokio::fs::read_to_string(&to_full).await {
        let doc = state.get_or_create_doc(&to);
        let mut txn = match doc.try_transact_mut() {
            Ok(txn) => txn,
            Err(_) => return super::circle_busy(),
        };
        let text = txn.get_or_insert_text(to.as_str());
        if !content.is_empty() {
            text.insert(&mut txn, 0, &content);
        }
    }
    let _ = state.interactive_writes.send((to.clone(), None));
    let _ = state
        .events
        .send(CircleEvent::FileUpdated { path: to.clone() });

    (
        StatusCode::OK,
        Json(json!({ "status": "renamed", "from": from, "to": to })),
    )
        .into_response()
}

pub async fn delete_file(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Json(req): Json<DeleteFileRequest>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "circle not found"})),
            )
                .into_response();
        }
    };
    let rel = match normalize_rel_path(&req.path) {
        Ok(path) => path,
        Err(error) => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
        }
    };
    let full = workspace_path(&state, &rel);
    if !tokio::fs::try_exists(&full).await.unwrap_or(false) {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "file not found" })),
        )
            .into_response();
    }
    if let Err(err) = tokio::fs::remove_file(&full).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        )
            .into_response();
    }

    state.remove_doc(&rel);
    crate::store::crdt::delete(&state.workspace, &rel).await;
    let _ = state.all_deletes.send(rel.clone());
    let _ = state.interactive_writes.send((rel.clone(), None));
    let _ = state
        .events
        .send(CircleEvent::FileDeleted { path: rel.clone() });

    (
        StatusCode::OK,
        Json(json!({ "status": "deleted", "path": rel })),
    )
        .into_response()
}
