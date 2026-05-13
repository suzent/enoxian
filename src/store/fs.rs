use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use yrs::{GetString, Transact};
use crate::state::AppState;

/// Flush a Y.Text document back to disk.
/// Sets a per-path self-write flag before writing so the file watcher
/// ignores the change and avoids a re-entrancy loop.
pub async fn flush_to_disk(
    state: &AppState,
    rel_path: &str,
    self_write_flag: &Arc<AtomicBool>,
) {
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

    self_write_flag.store(true, Ordering::SeqCst);
    let _ = tokio::fs::write(&full_path, &contents).await;
    // The flag will be cleared by the watcher after it sees and ignores the event.
}
