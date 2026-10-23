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
use yrs::{GetString, Text, Transact, WriteTxn};

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
    // The shared document first, then the file. The other way round, the
    // watcher can read the new file and fill the document before this does,
    // and inserting the content again here doubles it. Seeding only an empty
    // document makes the order safe either way.
    {
        let doc = state.get_or_create_doc(&rel);
        let mut txn = match doc.try_transact_mut() {
            Ok(txn) => txn,
            Err(_) => return super::circle_busy(),
        };
        let text = txn.get_or_insert_text(rel.as_str());
        if !content.is_empty() && text.len(&txn) == 0 {
            text.insert(&mut txn, 0, &content);
        }
    }
    if let Err(err) = tokio::fs::write(&full, &content).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        )
            .into_response();
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

/// Mark `path`, or every file under it, as written after `deleted_at` (ms):
/// now, or just past the tombstone when that is not later, as when the
/// deleting peer's clock runs ahead of ours. Reconcile spares only a file
/// strictly newer than the tombstone.
async fn mark_recreated(path: &std::path::Path, deleted_at: i64) {
    let after =
        std::time::UNIX_EPOCH + std::time::Duration::from_millis(deleted_at.max(0) as u64 + 1);
    let when = std::time::SystemTime::now().max(after);
    let mut stack = vec![path.to_path_buf()];
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            if let Ok(mut entries) = tokio::fs::read_dir(&path).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    stack.push(entry.path());
                }
            }
            continue;
        }
        let touched = std::fs::File::options()
            .write(true)
            .open(&path)
            .and_then(|file| file.set_modified(when));
        if let Err(error) = touched {
            tracing::warn!(
                "[files] could not mark {} re-created: {error}",
                path.display()
            );
        }
    }
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

    // The old path is a deletion like any other, tombstone included. Dropping
    // only the document here also hides the old path from the watcher, whose
    // rename event then finds nothing tracked there and records nothing; a
    // peer still holding the file brings it back on its next catch-up.
    crate::sync_yjs::watcher::record_removal(&state, &from).await;
    let _ = state.interactive_writes.send((from.clone(), None));

    // A move into a deleted place is a re-creation there. The tombstone may
    // name the destination or a folder above it; clearing a folder's would
    // bring back everything else deleted in it, so the moved file is marked
    // newer instead, which is what reconcile spares. A rename keeps the old
    // modification time, so without this the next reconcile deletes it.
    if let Some(deleted_at) = crate::deletions::covering_ts(&state, &to) {
        crate::deletions::clear(&state, &to);
        mark_recreated(&to_full, deleted_at).await;
    }

    if let Ok(content) = tokio::fs::read_to_string(&to_full).await {
        let doc = state.get_or_create_doc(&to);
        {
            let mut txn = match doc.try_transact_mut() {
                Ok(txn) => txn,
                Err(_) => return super::circle_busy(),
            };
            let text = txn.get_or_insert_text(to.as_str());
            // The watcher may have read the moved file first; inserting the
            // content again would double it.
            if text.get_string(&txn) != content {
                let len = text.len(&txn);
                if len > 0 {
                    text.remove_range(&mut txn, 0, len);
                }
                if !content.is_empty() {
                    text.insert(&mut txn, 0, &content);
                }
            }
        }
        crate::store::crdt::save(&state.workspace, &to, &doc).await;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use yrs::ReadTxn;

    fn circle() -> (DaemonState, AppState, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(
            "c1".into(),
            "circle".into(),
            dir.path().to_path_buf(),
            dir.path().join(".circle"),
            String::new(),
            "label".into(),
            1,
            "peer".into(),
            crate::config::JoinPolicy::Manual,
            "suzy".into(),
            crate::mls::new_mls_state(crate::mls::MlsIdentity::generate("peer").unwrap(), None),
        );
        let daemon = DaemonState::new();
        daemon.insert_circle(
            "c1".into(),
            state.clone(),
            tokio_util::sync::CancellationToken::new(),
        );
        (daemon, state, dir)
    }

    fn seed(state: &AppState, path: &str, content: &str) {
        let doc = state.get_or_create_doc(path);
        let mut txn = doc.transact_mut();
        let text = txn.get_or_insert_text(path);
        text.insert(&mut txn, 0, content);
    }

    fn text_of(state: &AppState, path: &str) -> Option<String> {
        let doc = state.docs.get(path)?.clone();
        let txn = doc.transact();
        Some(txn.get_text(path)?.get_string(&txn))
    }

    async fn rename(daemon: &DaemonState, from: &str, to: &str) -> StatusCode {
        rename_file(
            State(daemon.clone()),
            Path("c1".into()),
            Json(RenameFileRequest {
                from: from.into(),
                to: to.into(),
            }),
        )
        .await
        .into_response()
        .status()
    }

    /// The old path is tombstoned, so a peer that still has it cannot bring it
    /// back on its next catch-up.
    #[tokio::test]
    async fn a_renamed_file_leaves_a_tombstone_behind() {
        let (daemon, state, dir) = circle();
        std::fs::write(dir.path().join("ROADMAP.md"), "# plan\n").unwrap();
        seed(&state, "ROADMAP.md", "# plan\n");

        assert_eq!(
            rename(&daemon, "ROADMAP.md", "plan/roadmap.md").await,
            StatusCode::OK
        );

        assert!(crate::deletions::is_deleted(&state, "ROADMAP.md"));
        assert!(text_of(&state, "ROADMAP.md").is_none());
        assert_eq!(
            text_of(&state, "plan/roadmap.md").as_deref(),
            Some("# plan\n")
        );
        assert!(crate::store::crdt::state_path(&state.workspace, "plan/roadmap.md").exists());
    }

    /// The watcher can read the moved file before the API seeds its document.
    #[tokio::test]
    async fn a_document_the_watcher_already_filled_is_not_doubled() {
        let (daemon, state, dir) = circle();
        std::fs::write(dir.path().join("a.md"), "once\n").unwrap();
        seed(&state, "a.md", "once\n");
        seed(&state, "b.md", "once\n");

        assert_eq!(rename(&daemon, "a.md", "b.md").await, StatusCode::OK);
        assert_eq!(text_of(&state, "b.md").as_deref(), Some("once\n"));
    }

    /// A rename keeps the file's old modification time, so a move into a
    /// folder deleted since then would be taken for the deleted file, and so
    /// would any file not newer than a tombstone from a clock running ahead.
    #[tokio::test]
    async fn moving_into_a_deleted_folder_survives_reconcile() {
        let (daemon, state, dir) = circle();
        let old = dir.path().join("a.md");
        std::fs::write(&old, "x\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1))
            .unwrap();
        seed(&state, "a.md", "x\n");
        // Deleted by a peer whose clock runs an hour ahead of ours.
        {
            use yrs::Map;
            let deletion = crate::control::Deletion {
                path: "gone".into(),
                ts: crate::deletions::now_ms() + 3_600_000,
                peer_id: "ahead".into(),
            };
            let mut txn = state.control.transact_mut();
            let map = txn.get_or_insert_map(crate::control::DELETIONS_KEY);
            let json = serde_json::to_string(&deletion).unwrap();
            map.insert(&mut txn, "gone", yrs::Any::String(json.into()));
        }

        assert_eq!(rename(&daemon, "a.md", "gone/b.md").await, StatusCode::OK);
        crate::deletions::reconcile(&state).await;

        assert!(dir.path().join("gone/b.md").exists());
        assert_eq!(text_of(&state, "gone/b.md").as_deref(), Some("x\n"));
    }

    /// Moving a file onto a path deleted earlier must not leave that deletion
    /// standing, or the next reconcile would delete the moved file.
    #[tokio::test]
    async fn moving_onto_a_deleted_path_clears_its_tombstone() {
        let (daemon, state, dir) = circle();
        std::fs::write(dir.path().join("a.md"), "x\n").unwrap();
        seed(&state, "a.md", "x\n");
        crate::deletions::record(&state, "b.md");

        assert_eq!(rename(&daemon, "a.md", "b.md").await, StatusCode::OK);
        assert!(!crate::deletions::is_deleted(&state, "b.md"));
        assert!(crate::deletions::is_deleted(&state, "a.md"));
    }
}
