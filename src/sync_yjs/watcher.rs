use crate::control::CircleEvent;
use crate::state::AppState;
use notify::event::{CreateKind, ModifyKind, RemoveKind, RenameMode};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use yrs::{GetString, Text, Transact, WriteTxn};

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
            let rel = match path.strip_prefix(workspace) {
                Ok(r) => r.to_string_lossy().replace('\\', "/"),
                Err(_) => continue,
            };
            if state.is_ignored(&rel) {
                continue;
            }

            if path.is_dir() {
                stack.push(path);
            } else {
                let contents = match tokio::fs::read_to_string(&path).await {
                    Ok(c) => c,
                    Err(_) => continue, // skip binary files
                };
                let doc = state.get_or_create_doc(&rel);
                // Restore saved CRDT state first — preserves operation IDs from previous session
                // so merging with peers after restart is idempotent (no content duplication).
                let restored = crate::store::crdt::restore(&state.workspace, &rel, &doc).await;
                let changed = {
                    let mut txn = match doc.try_transact_mut() {
                        Ok(txn) => txn,
                        Err(_) => continue,
                    };
                    let text = txn.get_or_insert_text(rel.as_str());
                    let current = text.get_string(&txn);
                    if current == contents {
                        false
                    } else {
                        // File was edited while daemon was offline — apply the diff.
                        // This creates new ops, but only happens for genuine offline edits.
                        let len = text.len(&txn);
                        if len > 0 {
                            text.remove_range(&mut txn, 0, len);
                        }
                        if !contents.is_empty() {
                            text.insert(&mut txn, 0, &contents);
                        }
                        true
                    }
                };
                if changed || !restored {
                    // Persist the bootstrapped/offline-edited state immediately.
                    // Without this, a restart can re-seed identical file text with
                    // fresh Yjs operation IDs, which later merge as duplicate text.
                    crate::store::crdt::save(&state.workspace, &rel, &doc).await;
                }
                tracing::debug!("[preload] loaded '{rel}' (crdt restored: {restored})");
            }
        }
    }
}

/// Spawn the file-system watcher task.
pub async fn spawn_watcher(
    state: AppState,
    workspace: PathBuf,
    token: CancellationToken,
) -> anyhow::Result<()> {
    // Compile ignore rules before the preload scan, so a build tree is never
    // tracked in the first place.
    state.reload_ignore_rules();

    preload_workspace(&state, &workspace).await;

    // Rules can change between runs — a `.gitignore` added, or a new built-in
    // shipped. Drop anything now excluded. Untracking only: the files stay on
    // disk here and on every peer, because adding an ignore rule is not a
    // request to delete a teammate's build output.
    let now_ignored: Vec<String> = state
        .docs
        .iter()
        .map(|e| e.key().clone())
        .filter(|p| state.is_ignored(p))
        .collect();
    if !now_ignored.is_empty() {
        tracing::info!(
            "[watcher] untracking {} newly-ignored path(s)",
            now_ignored.len()
        );
        for path in now_ignored {
            state.remove_doc(&path);
            crate::store::crdt::delete(&state.workspace, &path).await;
        }
    }

    // Preload re-creates a doc for every file on disk, including ones a peer
    // deleted while this device was offline. Apply pending tombstones now, so
    // those files are removed instead of being advertised back to the peer that
    // deleted them. Prune first so expired tombstones do not resurrect work.
    crate::deletions::prune(&state);
    let removed = crate::deletions::reconcile(&state).await;
    if removed > 0 {
        tracing::info!("[watcher] applied {removed} deletion(s) recorded while offline");
    }

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
            | EventKind::Modify(ModifyKind::Name(_))  // To, From, Both, Any — covers macOS atomic renames
            | EventKind::Create(CreateKind::File)
            | EventKind::Create(CreateKind::Any)
            | EventKind::Remove(RemoveKind::File)
            | EventKind::Remove(RemoveKind::Any)
    );
    if !relevant {
        return;
    }

    // For rename events, only process the destination (last path).
    // Name(From) carries the source that moved away — skip it.
    // Name(Both) carries [source, destination] — we only want the destination.
    let paths: &[_] = match event.kind {
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => &[],
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
            event.paths.last().map(std::slice::from_ref).unwrap_or(&[])
        }
        _ => &event.paths,
    };

    for path in paths {
        let rel = match path.strip_prefix(workspace) {
            Ok(r) => r.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };

        // Recompile before the ignore check: an ignore file is itself a
        // dotfile, so testing it first would discard the very event that tells
        // us the rules changed. Not tracked either way — only consulted.
        if crate::ignore_rules::is_ignore_file(&rel) {
            state.reload_ignore_rules();
        }

        if state.is_ignored(&rel) {
            continue;
        }

        if matches!(event.kind, EventKind::Remove(_)) {
            // One event can stand for a whole tree. Deleting a folder is
            // reported per-file on some platforms and as a single event for the
            // directory on others (moving a folder to the Trash is one rename
            // of the folder). Expanding to the documents beneath the path makes
            // both shapes produce the same durable result, instead of a
            // tombstone keyed `repo` that matches none of the 1713 documents
            // keyed `repo/...`.
            let mut paths = crate::deletions::docs_under(state, &rel);
            if !paths.contains(&rel) {
                paths.push(rel.clone());
            }

            // Durable record first: the live frame below only reaches peers
            // connected at this instant, and a bulk delete overflows its
            // broadcast buffer. The tombstone is what makes the deletion
            // survive a disconnect and stop the file being re-created here on
            // the next handshake.
            crate::deletions::record(state, &rel);

            for path in paths {
                state.remove_doc(&path);
                crate::store::crdt::delete(&state.workspace, &path).await;
                crate::deletions::record(state, &path);
                let _ = state.all_deletes.send(path.clone());
                let _ = state.events.send(CircleEvent::FileDeleted { path });
            }
            continue;
        }

        // The path exists, so any tombstone for it is obsolete. Clearing it
        // here is what makes delete-then-recreate work; otherwise the next
        // reconcile would delete the new file.
        if crate::deletions::is_deleted(state, &rel) {
            crate::deletions::clear(state, &rel);
        }

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
        let changed = {
            let mut txn = match doc.try_transact_mut() {
                Ok(txn) => txn,
                Err(_) => continue,
            };
            let text = txn.get_or_insert_text(rel.as_str());
            let current = text.get_string(&txn);
            if current != contents {
                let len = text.len(&txn);
                if len > 0 {
                    text.remove_range(&mut txn, 0, len);
                }
                if !contents.is_empty() {
                    text.insert(&mut txn, 0, &contents);
                }
                true
            } else {
                false
            }
        };

        // Save CRDT state after a local edit so restarts see the correct state.
        if changed {
            crate::store::crdt::save(&state.workspace, &rel, &doc).await;
        }

        let _ = state.events.send(CircleEvent::FileUpdated { path: rel });
    }
}
