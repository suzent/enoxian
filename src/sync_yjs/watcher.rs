use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use notify::event::{CreateKind, ModifyKind, RenameMode};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use yrs::{GetString, Text, Transact};
use crate::control::CircleEvent;
use crate::state::AppState;

/// Pre-load all existing files in the workspace into the CRDT.
/// Must run before the watcher starts so that any file present before daemon
/// startup is included in the P2P handshake's doc set.
pub async fn preload_workspace(state: &AppState, workspace: &PathBuf) {
    let mut stack = vec![workspace.clone()];
    while let Some(dir) = stack.pop() {
        let mut rd = match tokio::fs::read_dir(&dir).await {
            Ok(r) => r,
            Err(_) => continue,
        };
        while let Ok(Some(entry)) = rd.next_entry().await {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = match path.strip_prefix(workspace) {
                    Ok(r) => r.to_string_lossy().replace('\\', "/"),
                    Err(_) => continue,
                };
                let contents = match tokio::fs::read_to_string(&path).await {
                    Ok(c) => c,
                    Err(_) => continue, // skip binary files
                };
                let doc = state.get_or_create_doc(&rel);
                let text = doc.get_or_insert_text(rel.as_str());
                let mut txn = doc.transact_mut();
                let current = text.get_string(&txn);
                if current != contents {
                    let len = text.len(&txn);
                    if len > 0 { text.remove_range(&mut txn, 0, len); }
                    if !contents.is_empty() { text.insert(&mut txn, 0, &contents); }
                }
                tracing::debug!("[preload] loaded '{rel}'");
            }
        }
    }
}

/// Spawn the file-system watcher task.
pub async fn spawn_watcher(state: AppState, workspace: PathBuf, token: CancellationToken) -> anyhow::Result<()> {
    preload_workspace(&state, &workspace).await;

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
        loop {
            tokio::select! {
                _ = token.cancelled() => break,
                result = tokio_rx.recv() => match result {
                    Some(Ok(event)) => handle_event(&state, &workspace, event).await,
                    Some(Err(e)) => tracing::warn!("watcher error: {e}"),
                    None => break,
                }
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
            }
        }

        let _ = state.events.send(CircleEvent::FileUpdated { path: rel });
    }
}
