use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use notify::event::{CreateKind, ModifyKind, RenameMode};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;
use yrs::{GetString, Text, Transact};
use crate::control::CircleEvent;
use crate::state::AppState;

/// Spawn the file-system watcher task.
pub async fn spawn_watcher(state: AppState, workspace: PathBuf) -> anyhow::Result<()> {
    let (tokio_tx, mut tokio_rx) = mpsc::channel::<notify::Result<Event>>(128);
    let (std_tx, std_rx) = std::sync::mpsc::channel::<notify::Result<Event>>();

    // Bridge: std::mpsc → tokio::mpsc (runs in a blocking thread)
    let bridge_tx = tokio_tx.clone();
    std::thread::spawn(move || {
        while let Ok(evt) = std_rx.recv() {
            if bridge_tx.blocking_send(evt).is_err() {
                break;
            }
        }
    });

    let mut watcher = RecommendedWatcher::new(std_tx, Config::default())?;
    tokio::fs::create_dir_all(&workspace).await?;
    watcher.watch(&workspace, RecursiveMode::Recursive)?;

    tokio::spawn(async move {
        let _watcher = watcher; // keep alive inside task
        while let Some(result) = tokio_rx.recv().await {
            match result {
                Ok(event) => handle_event(&state, &workspace, event).await,
                Err(e) => tracing::warn!("watcher error: {e}"),
            }
        }
    });

    Ok(())
}

async fn handle_event(state: &AppState, workspace: &PathBuf, event: Event) {
    let relevant = matches!(
        event.kind,
        EventKind::Modify(ModifyKind::Data(_))
            | EventKind::Modify(ModifyKind::Any)
            | EventKind::Modify(ModifyKind::Name(RenameMode::To))
            | EventKind::Create(CreateKind::File)
            | EventKind::Create(CreateKind::Any)
    );
    if !relevant {
        return;
    }

    for path in &event.paths {
        let rel = match path.strip_prefix(workspace) {
            Ok(r) => r.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };

        // Check the shared self_write_flag. If flush_to_disk set it, this event
        // was caused by a P2P or WS write — skip it to avoid a re-entrancy loop.
        let flag = state
            .self_write_flags
            .entry(rel.clone())
            .or_insert_with(|| Arc::new(AtomicBool::new(false)))
            .clone();

        if flag.swap(false, Ordering::SeqCst) {
            continue; // self-write — ignore
        }

        let contents = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(_) => continue,
        };

        tracing::info!("[watcher] detected change: '{rel}' ({} bytes)", contents.len());

        // Apply to Y.Text (full replace — last external writer wins).
        // The observer fires on TransactionMut drop → broadcasts to doc_updates + all_updates.
        let doc = state.get_or_create_doc(&rel);
        {
            let text = doc.get_or_insert_text(rel.as_str());
            let mut txn = doc.transact_mut();
            let current = text.get_string(&txn);
            if current != contents {
                let len = text.len(&txn);
                if len > 0 {
                    text.remove_range(&mut txn, 0, len);
                }
                if !contents.is_empty() {
                    text.insert(&mut txn, 0, &contents);
                }
                tracing::info!("[watcher] CRDT updated for '{rel}', broadcasting");
            } else {
                tracing::info!("[watcher] '{rel}' content unchanged — skipping CRDT write");
            }
        }

        let _ = state.events.send(CircleEvent::FileUpdated { path: rel });
    }
}
