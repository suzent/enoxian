use crate::state::AppState;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use yrs::{GetString, ReadTxn, Transact};

/// Flush a Y.Text document back to disk.
/// Sets the shared per-path self_write_flag before writing so the file watcher
/// ignores the change and avoids a re-entrancy loop.
/// `author` is `None` for local writes and `Some(device_label)` for P2P peer writes.
pub async fn flush_to_disk(state: &AppState, rel_path: &str, author: Option<String>) {
    let doc = match state.docs.get(rel_path) {
        Some(d) => d.clone(),
        None => return,
    };

    let contents = {
        let txn = match doc.try_transact() {
            Ok(txn) => txn,
            Err(_) => {
                tracing::debug!("[fs] state busy; deferring flush for {rel_path}");
                return;
            }
        };
        txn.get_text(rel_path)
            .map(|text| text.get_string(&txn))
            .unwrap_or_default()
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
    let _ = state
        .interactive_writes
        .send((rel_path.to_string(), author));
    // Save CRDT state immediately after the file write — guarantees the saved state
    // always matches the file. Doing this here (awaited, not spawned) prevents the
    // race where a background save is killed on shutdown, leaving a stale CRDT state
    // that causes content duplication when the daemon restarts and syncs with peers.
    crate::store::crdt::save(&state.workspace, rel_path, &doc).await;
    // The watcher clears the flag when it sees and ignores the resulting fs event.
}
