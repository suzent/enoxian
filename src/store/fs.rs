use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use yrs::{GetString, Transact};
use crate::state::AppState;

/// Flush a Y.Text document back to disk.
/// Sets the shared per-path self_write_flag before writing so the file watcher
/// ignores the change and avoids a re-entrancy loop.
pub async fn flush_to_disk(state: &AppState, rel_path: &str) {
    let doc = match state.docs.get(rel_path) {
        Some(d) => d.clone(),
        None => return,
    };

    let contents = {
        let text = doc.get_or_insert_text(rel_path);
        let txn = doc.transact();
        text.get_string(&txn)
    };

    let full_path = state
        .workspace
        .join(rel_path.replace('/', std::path::MAIN_SEPARATOR_STR));

    if let Some(parent) = full_path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }

    let flag = state
        .self_write_flags
        .entry(rel_path.to_string())
        .or_insert_with(|| Arc::new(AtomicBool::new(false)))
        .clone();

    flag.store(true, Ordering::SeqCst);
    let _ = tokio::fs::write(&full_path, &contents).await;
    // Save CRDT state immediately after the file write — guarantees the saved state
    // always matches the file. Doing this here (awaited, not spawned) prevents the
    // race where a background save is killed on shutdown, leaving a stale CRDT state
    // that causes content duplication when the daemon restarts and syncs with peers.
    crate::store::crdt::save(&state.workspace, rel_path, &doc).await;
    // The watcher clears the flag when it sees and ignores the resulting fs event.
}
