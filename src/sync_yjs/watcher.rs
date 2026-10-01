use crate::control::CircleEvent;
use crate::state::AppState;
use notify::event::{CreateKind, ModifyKind};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
        let mut sweep = tokio::time::interval(SWEEP_INTERVAL);
        sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        sweep.tick().await; // the first tick is immediate; preload just ran
        let mut unloadable = Unloadable::new();
        loop {
            tokio::select! {
                _ = token.cancelled() => break,
                result = tokio_rx.recv() => match result {
                    Some(Ok(event)) if event.need_rescan() => {
                        tracing::warn!("[watcher] events were dropped; rescanning the workspace");
                        sweep_untracked(&state, &workspace, &mut unloadable).await;
                    }
                    Some(Ok(event)) => handle_event(&state, &workspace, event).await,
                    Some(Err(e)) => tracing::warn!("watcher error: {e}"),
                    None => break,
                },
                _ = sweep.tick() => {
                    sweep_untracked(&state, &workspace, &mut unloadable).await;
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
            | EventKind::Remove(_)
    );
    if !relevant {
        return;
    }
    let is_rename = matches!(event.kind, EventKind::Modify(ModifyKind::Name(_)));
    let is_remove = matches!(event.kind, EventKind::Remove(_));
    let is_create = matches!(event.kind, EventKind::Create(_));

    // Every path is processed, rename sources included. Whether a rename path
    // is the side that moved away or the side that arrived is decided below by
    // whether it still exists: the mode is not enough, because macOS reports
    // both sides as Name(Any), and moving a folder to the Trash or out of the
    // workspace reports only the side that left.
    for path in &event.paths {
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

        if (is_remove || is_rename) && !tokio::fs::try_exists(path).await.unwrap_or(true) {
            // A rename away from a path nothing tracks is an editor's temp
            // file after an atomic save; tombstoning those would only grow the
            // control doc.
            if is_rename && crate::deletions::docs_under(state, &rel).is_empty() {
                continue;
            }
            record_removal(state, &rel).await;
            continue;
        }

        // A directory event stands for its whole tree: a folder renamed or
        // moved in arrives as one event for the folder, and a folder removed
        // and created again may hold only some of what was tracked under it.
        // Data and attribute changes on a directory say nothing about its
        // contents, so those are not rescanned.
        if (is_remove || is_rename || is_create)
            && tokio::fs::metadata(path)
                .await
                .map(|m| m.is_dir())
                .unwrap_or(false)
        {
            sync_directory(state, workspace, path, &rel).await;
            continue;
        }

        ingest_file(state, path, rel).await;
    }
}

/// How often the workspace is checked for files no event ever reported.
const SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// A file this recent may still be mid-copy; the next sweep takes it whole.
const SWEEP_SETTLE: std::time::Duration = std::time::Duration::from_secs(5);

/// Files a sweep could not load (binary, or unreadable), with the size and
/// mtime seen, so an unchanged one is not read again on every sweep.
type Unloadable = HashMap<String, (u64, std::time::SystemTime)>;

/// Start tracking files that are on disk but were never reported.
///
/// File events are not guaranteed. On Windows, notify drops a whole batch
/// without any signal when the change buffer overflows (a large copy), and a
/// file still locked by the copier fails to read on the one event it gets.
/// Without this, a file missed that way stays untracked until the daemon
/// restarts, invisible to every peer. Only adds: a tracked file missing from
/// disk may be a peer's write not yet flushed, so removals stay event-driven.
async fn sweep_untracked(state: &AppState, workspace: &Path, unloadable: &mut Unloadable) {
    let now = std::time::SystemTime::now();
    let mut picked_up = 0usize;
    let mut stack = vec![workspace.to_path_buf()];
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
            let Ok(meta) = entry.metadata().await else {
                continue;
            };
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            // A tombstoned file is the deletion reconcile's to decide.
            if state.docs.contains_key(&rel) || crate::deletions::is_deleted(state, &rel) {
                continue;
            }
            let Ok(modified) = meta.modified() else {
                continue;
            };
            if now.duration_since(modified).unwrap_or_default() < SWEEP_SETTLE {
                continue;
            }
            let seen = (meta.len(), modified);
            if unloadable.get(&rel) == Some(&seen) {
                continue;
            }
            ingest_file(state, &path, rel.clone()).await;
            if state.docs.contains_key(&rel) {
                unloadable.remove(&rel);
                picked_up += 1;
            } else {
                unloadable.insert(rel, seen);
            }
        }
    }
    if picked_up > 0 {
        tracing::info!("[watcher] picked up {picked_up} file(s) no event reported");
    }
}

/// Bring the tracked documents under directory `rel` in line with its contents
/// on disk: untrack what is no longer there, and load every file that is.
async fn sync_directory(state: &AppState, workspace: &Path, dir: &Path, rel: &str) {
    // A tombstone on the directory itself would delete everything loaded below
    // on the next reconcile; the directory exists again, so it is obsolete.
    if crate::deletions::get(state, rel).is_some() {
        crate::deletions::clear(state, rel);
    }

    for tracked in crate::deletions::docs_under(state, rel) {
        let full = workspace.join(tracked.replace('/', std::path::MAIN_SEPARATOR_STR));
        if !tokio::fs::try_exists(&full).await.unwrap_or(true) {
            record_removal(state, &tracked).await;
        }
    }

    let mut stack = vec![dir.to_path_buf()];
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
                ingest_file(state, &path, rel).await;
            }
        }
    }
}

/// Load the file at `path` into its document, replacing the text if it changed.
async fn ingest_file(state: &AppState, path: &Path, rel: String) {
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
        return; // self-write — ignore
    }

    let contents = match tokio::fs::read_to_string(path).await {
        Ok(c) => c,
        Err(_) => return,
    };

    // Apply to Y.Text (full replace — last external writer wins).
    // The observer fires on TransactionMut drop → broadcasts to doc_updates + all_updates.
    let doc = state.get_or_create_doc(&rel);
    let changed = {
        let mut txn = match doc.try_transact_mut() {
            Ok(txn) => txn,
            Err(_) => return,
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
        if let Some(holder) =
            crate::control::arbitration::holder_elsewhere(&state.control, &rel, &state.peer_id)
        {
            let _ = state.events.send(CircleEvent::LockViolated {
                path: rel.clone(),
                held_by: holder.agent_id,
                held_by_peer_id: holder.peer_id,
                edited_by_peer_id: state.peer_id.clone(),
            });
        }
    }

    let _ = state.events.send(CircleEvent::FileUpdated { path: rel });
}

/// Untrack `rel` and everything beneath it, and tell peers it is gone.
async fn record_removal(state: &AppState, rel: &str) {
    // One event can stand for a whole tree. Deleting a folder is reported
    // per-file on some platforms and as a single event for the directory on
    // others (moving a folder to the Trash is one rename of the folder).
    // Expanding to the documents beneath the path makes both shapes produce the
    // same durable result, instead of a tombstone keyed `repo` that matches
    // none of the 1713 documents keyed `repo/...`.
    let mut paths = crate::deletions::docs_under(state, rel);
    if !paths.iter().any(|p| p == rel) {
        paths.push(rel.to_string());
    }

    // Durable record first: the live frame below only reaches peers connected
    // at this instant, and a bulk delete overflows its broadcast buffer. The
    // tombstone is what makes the deletion survive a disconnect and stop the
    // file being re-created here on the next handshake.
    crate::deletions::record(state, rel);

    for path in paths {
        state.remove_doc(&path);
        crate::store::crdt::delete(&state.workspace, &path).await;
        crate::deletions::record(state, &path);
        let _ = state.all_deletes.send(path.clone());
        let _ = state.events.send(CircleEvent::FileDeleted { path });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::JoinPolicy, mls};
    use notify::event::{RemoveKind, RenameMode};

    fn test_state(workspace: PathBuf) -> AppState {
        AppState::new(
            "circle".into(),
            "Circle".into(),
            workspace,
            PathBuf::new(),
            String::new(),
            "agent".into(),
            1,
            "peer-local".into(),
            JoinPolicy::Manual,
            "owner".into(),
            mls::new_mls_state(mls::MlsIdentity::generate("peer-local").unwrap(), None),
        )
    }

    fn event(kind: EventKind, paths: Vec<PathBuf>) -> Event {
        Event {
            kind,
            paths,
            attrs: Default::default(),
        }
    }

    /// macOS reports moving a folder to the Trash as one Name(Any) for the
    /// folder, and nothing for the files inside it.
    #[tokio::test]
    async fn moving_a_folder_out_deletes_every_file_under_it() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().to_path_buf();
        let state = test_state(workspace.clone());
        for p in ["source/a.rs", "source/src/b.rs", "sourced.rs"] {
            state.get_or_create_doc(p);
        }
        std::fs::write(workspace.join("sourced.rs"), "keep").unwrap();

        let mut deletes = state.all_deletes.subscribe();
        handle_event(
            &state,
            &workspace,
            event(
                EventKind::Modify(ModifyKind::Name(RenameMode::Any)),
                vec![workspace.join("source")],
            ),
        )
        .await;

        assert!(!state.docs.contains_key("source/a.rs"));
        assert!(!state.docs.contains_key("source/src/b.rs"));
        assert!(state.docs.contains_key("sourced.rs"));
        assert!(crate::deletions::is_deleted(&state, "source/src/b.rs"));
        assert!(!crate::deletions::is_deleted(&state, "sourced.rs"));

        let mut sent = Vec::new();
        while let Ok(p) = deletes.try_recv() {
            sent.push(p);
        }
        sent.sort();
        assert_eq!(sent, vec!["source", "source/a.rs", "source/src/b.rs"]);
    }

    #[tokio::test]
    async fn a_rename_within_the_workspace_untracks_the_old_name() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().to_path_buf();
        let state = test_state(workspace.clone());
        state.get_or_create_doc("old.md");
        std::fs::write(workspace.join("new.md"), "body").unwrap();

        handle_event(
            &state,
            &workspace,
            event(
                EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
                vec![workspace.join("old.md"), workspace.join("new.md")],
            ),
        )
        .await;

        assert!(!state.docs.contains_key("old.md"));
        assert!(crate::deletions::is_deleted(&state, "old.md"));
        assert!(state.docs.contains_key("new.md"));
    }

    /// An editor's atomic save renames a temp file over the real one. The
    /// temp file was never tracked, so it must not leave a tombstone behind.
    #[tokio::test]
    async fn renaming_away_an_untracked_temp_file_records_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().to_path_buf();
        let state = test_state(workspace.clone());

        handle_event(
            &state,
            &workspace,
            event(
                EventKind::Modify(ModifyKind::Name(RenameMode::From)),
                vec![workspace.join("notes.md.tmp")],
            ),
        )
        .await;

        assert!(!crate::deletions::is_deleted(&state, "notes.md.tmp"));
    }

    #[tokio::test]
    async fn removing_a_folder_deletes_every_file_under_it() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().to_path_buf();
        let state = test_state(workspace.clone());
        state.get_or_create_doc("repo/a.rs");

        handle_event(
            &state,
            &workspace,
            event(
                EventKind::Remove(RemoveKind::Folder),
                vec![workspace.join("repo")],
            ),
        )
        .await;

        assert!(!state.docs.contains_key("repo/a.rs"));
        assert!(crate::deletions::is_deleted(&state, "repo/a.rs"));
    }

    /// Coalesced events can report a removal for a path that has since been
    /// written again; the file on disk is the truth.
    #[tokio::test]
    async fn a_remove_event_for_a_path_that_exists_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().to_path_buf();
        let state = test_state(workspace.clone());
        std::fs::write(workspace.join("a.md"), "still here").unwrap();
        state.get_or_create_doc("a.md");

        handle_event(
            &state,
            &workspace,
            event(
                EventKind::Remove(RemoveKind::File),
                vec![workspace.join("a.md")],
            ),
        )
        .await;

        assert!(state.docs.contains_key("a.md"));
        assert!(!crate::deletions::is_deleted(&state, "a.md"));
    }

    #[tokio::test]
    async fn renaming_a_folder_keeps_its_files_under_the_new_name() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().to_path_buf();
        let state = test_state(workspace.clone());
        state.get_or_create_doc("old/a.md");
        state.get_or_create_doc("old/sub/b.md");
        std::fs::create_dir_all(workspace.join("new/sub")).unwrap();
        std::fs::write(workspace.join("new/a.md"), "a").unwrap();
        std::fs::write(workspace.join("new/sub/b.md"), "b").unwrap();

        handle_event(
            &state,
            &workspace,
            event(
                EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
                vec![workspace.join("old"), workspace.join("new")],
            ),
        )
        .await;

        assert!(!state.docs.contains_key("old/a.md"));
        assert!(!state.docs.contains_key("old/sub/b.md"));
        assert!(state.docs.contains_key("new/a.md"));
        assert!(state.docs.contains_key("new/sub/b.md"));
        assert!(!crate::deletions::is_deleted(&state, "new/sub/b.md"));
    }

    /// A Remove(Folder) handled after the folder was created again must still
    /// untrack whatever the new folder no longer holds.
    #[tokio::test]
    async fn a_recreated_folder_untracks_files_it_no_longer_holds() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().to_path_buf();
        let state = test_state(workspace.clone());
        state.get_or_create_doc("repo/gone.md");
        state.get_or_create_doc("repo/kept.md");
        std::fs::create_dir_all(workspace.join("repo")).unwrap();
        std::fs::write(workspace.join("repo/kept.md"), "kept").unwrap();

        handle_event(
            &state,
            &workspace,
            event(
                EventKind::Remove(RemoveKind::Folder),
                vec![workspace.join("repo")],
            ),
        )
        .await;

        assert!(!state.docs.contains_key("repo/gone.md"));
        assert!(crate::deletions::is_deleted(&state, "repo/gone.md"));
        assert!(state.docs.contains_key("repo/kept.md"));
        assert!(!crate::deletions::is_deleted(&state, "repo/kept.md"));
    }

    fn write_settled(path: &Path, bytes: &[u8]) {
        std::fs::write(path, bytes).unwrap();
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(past)
            .unwrap();
    }

    /// The files in this test were never reported by any event, which is what
    /// a dropped Windows batch looks like.
    #[tokio::test]
    async fn a_sweep_tracks_files_no_event_reported() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().to_path_buf();
        let state = test_state(workspace.clone());
        std::fs::create_dir_all(workspace.join("notes")).unwrap();
        write_settled(&workspace.join("plan.md"), b"plan");
        write_settled(&workspace.join("notes/review.md"), b"review");
        state.get_or_create_doc("tracked.md");

        let mut unloadable = Unloadable::new();
        sweep_untracked(&state, &workspace, &mut unloadable).await;

        assert!(state.docs.contains_key("plan.md"));
        assert!(state.docs.contains_key("notes/review.md"));
        assert!(state.docs.contains_key("tracked.md"));
    }

    #[tokio::test]
    async fn a_sweep_leaves_fresh_tombstoned_and_ignored_files_alone() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().to_path_buf();
        let state = test_state(workspace.clone());
        // Still being written, as far as the sweep can tell.
        std::fs::write(workspace.join("copying.md"), b"partial").unwrap();
        write_settled(&workspace.join("gone.md"), b"deleted by a peer");
        crate::deletions::record(&state, "gone.md");
        std::fs::create_dir_all(workspace.join("node_modules/x")).unwrap();
        write_settled(&workspace.join("node_modules/x/index.js"), b"build");

        let mut unloadable = Unloadable::new();
        sweep_untracked(&state, &workspace, &mut unloadable).await;

        assert!(!state.docs.contains_key("copying.md"));
        assert!(!state.docs.contains_key("gone.md"));
        assert!(crate::deletions::is_deleted(&state, "gone.md"));
        assert!(!state.docs.contains_key("node_modules/x/index.js"));
    }

    #[tokio::test]
    async fn a_binary_file_is_not_reread_until_it_changes() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().to_path_buf();
        let state = test_state(workspace.clone());
        write_settled(&workspace.join("image.bin"), &[0xff, 0xfe, 0x00, 0x80]);

        let mut unloadable = Unloadable::new();
        sweep_untracked(&state, &workspace, &mut unloadable).await;
        assert!(!state.docs.contains_key("image.bin"));
        assert!(unloadable.contains_key("image.bin"));

        // Rewritten as text: the size and mtime change, so it is read again.
        write_settled(&workspace.join("image.bin"), b"now text");
        sweep_untracked(&state, &workspace, &mut unloadable).await;
        assert!(state.docs.contains_key("image.bin"));
        assert!(!unloadable.contains_key("image.bin"));
    }
}
