use std::path::{Path, PathBuf};
use std::sync::Arc;
use yrs::updates::decoder::Decode;
use yrs::{Doc, ReadTxn, StateVector, Transact, Update};

/// Path where the binary CRDT state for a doc is persisted.
/// Hidden directory so the watcher ignores it (`.` prefix rule).
pub fn state_path(workspace: &Path, rel_path: &str) -> PathBuf {
    workspace
        .join(".enox_crdt")
        .join(rel_path.replace('/', std::path::MAIN_SEPARATOR_STR))
}

/// Encode and write the full CRDT state for a doc.
pub async fn save(workspace: &Path, rel_path: &str, doc: &Arc<Doc>) {
    let bytes = doc.transact().encode_diff_v1(&StateVector::default());
    let path = state_path(workspace, rel_path);
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let _ = tokio::fs::write(path, bytes).await;
}

/// Load and apply a previously saved CRDT state into a fresh doc.
/// Returns true if state was found and applied.
pub async fn restore(workspace: &Path, rel_path: &str, doc: &Arc<Doc>) -> bool {
    let path = state_path(workspace, rel_path);
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(_) => return false,
    };
    let update = match Update::decode_v1(&bytes) {
        Ok(u) => u,
        Err(_) => return false,
    };
    doc.transact_mut_with("restore")
        .apply_update(update)
        .is_ok()
}

pub async fn delete(workspace: &Path, rel_path: &str) {
    let _ = tokio::fs::remove_file(state_path(workspace, rel_path)).await;
}
