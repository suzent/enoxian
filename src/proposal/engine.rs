//! Ambient proposal engine.
//!
//! Subscribes to the circle event stream (`FileUpdated` / `FileDeleted`,
//! which the watcher emits only for external disk edits — self-writes from
//! WS/P2P sync are suppressed upstream) and groups dirty paths into
//! proposals when an idle window closes:
//!
//! ```text
//! baseline S0 (taken at startup)
//!   -> file events mark paths dirty
//!   -> no events for IDLE_WINDOW
//!   -> dirty paths re-read from disk into snapshot S1
//!   -> diff S0 -> S1 becomes an ambient proposal
//!   -> S1 becomes the new baseline
//! ```
//!
//! Offline edits are handled at startup: if a baseline exists from a previous
//! run and the workspace differs from it, that difference becomes a proposal
//! before the live loop starts.

use super::diff::SnapshotDiff;
use super::model::Proposal;
use super::snapshot::{FileEntry, Snapshot};
use super::store::ProposalStore;
use crate::control::CircleEvent;
use crate::state::AppState;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

/// A dirty window closes after this long without further file events.
const IDLE_WINDOW: Duration = Duration::from_secs(3);

pub fn spawn_engine(state: AppState, token: CancellationToken) {
    tokio::spawn(async move {
        if let Err(e) = run(state, token).await {
            tracing::warn!("[proposal] engine stopped: {e:#}");
        }
    });
}

async fn run(state: AppState, token: CancellationToken) -> anyhow::Result<()> {
    let store = ProposalStore::open(&state.workspace)?;

    // Establish the baseline. A pre-existing baseline whose content differs
    // from the current workspace means offline edits — propose them.
    let disk = snapshot_workspace(&state, &store)?;
    let mut baseline = match store.baseline_id().and_then(|id| store.load_snapshot(&id).ok()) {
        Some(prev) => {
            let diff = SnapshotDiff::between(&prev, &disk);
            if diff.is_empty() {
                prev
            } else {
                store.save_snapshot(&disk)?;
                create_proposal(&state, &store, &prev, &disk, diff)?;
                disk
            }
        }
        None => {
            store.save_snapshot(&disk)?;
            store.set_baseline(&disk.id)?;
            disk
        }
    };
    tracing::info!("[proposal] engine started, baseline {}", baseline.id);

    let mut events = state.events.subscribe();
    let mut dirty: BTreeSet<String> = BTreeSet::new();
    let mut rescan = false;

    loop {
        tokio::select! {
            _ = token.cancelled() => break,
            evt = events.recv() => match evt {
                Ok(CircleEvent::FileUpdated { path }) | Ok(CircleEvent::FileDeleted { path }) => {
                    dirty.insert(path);
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("[proposal] event stream lagged by {n}, scheduling full rescan");
                    rescan = true;
                    // Force the idle branch to run even with no named dirty paths.
                    dirty.insert(String::new());
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            // Re-armed on every loop iteration, so this fires only after
            // IDLE_WINDOW of event silence — a debounce since the last event.
            _ = tokio::time::sleep(IDLE_WINDOW), if !dirty.is_empty() => {
                let result = if rescan {
                    snapshot_workspace(&state, &store)?
                } else {
                    snapshot_dirty(&state, &store, &baseline, &dirty)?
                };
                rescan = false;
                dirty.clear();

                let diff = SnapshotDiff::between(&baseline, &result);
                if diff.is_empty() {
                    continue;
                }
                store.save_snapshot(&result)?;
                create_proposal(&state, &store, &baseline, &result, diff)?;
                baseline = result;
            }
        }
    }
    Ok(())
}

fn create_proposal(
    state: &AppState,
    store: &ProposalStore,
    base: &Snapshot,
    result: &Snapshot,
    diff: SnapshotDiff,
) -> anyhow::Result<()> {
    let proposal = Proposal::ambient(
        state.circle_id.clone(),
        base.id.clone(),
        result.id.clone(),
        diff.changed_paths(),
    );
    store.save_proposal(&proposal)?;
    store.set_baseline(&result.id)?;
    tracing::info!(
        "[proposal] created {} ({} paths)",
        proposal.id,
        proposal.changed_paths.len()
    );
    let _ = state.events.send(CircleEvent::ProposalCreated {
        proposal_id: proposal.id,
    });
    Ok(())
}

/// Full workspace walk — used for the startup baseline and lag recovery.
fn snapshot_workspace(state: &AppState, store: &ProposalStore) -> anyhow::Result<Snapshot> {
    let mut files = BTreeMap::new();
    let mut stack = vec![state.workspace.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            let rel = match path.strip_prefix(&state.workspace) {
                Ok(r) => r.to_string_lossy().replace('\\', "/"),
                Err(_) => continue,
            };
            if crate::sync_yjs::watcher::is_ignored(&rel) {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(data) = std::fs::read(&path) {
                let hash = store.blobs.put(&data)?;
                files.insert(rel, FileEntry { hash, size: data.len() as u64 });
            }
        }
    }
    Ok(Snapshot::new(files))
}

/// Cheap incremental snapshot: clone the baseline manifest and re-read only
/// the dirty paths from disk.
fn snapshot_dirty(
    state: &AppState,
    store: &ProposalStore,
    baseline: &Snapshot,
    dirty: &BTreeSet<String>,
) -> anyhow::Result<Snapshot> {
    let mut files = baseline.files.clone();
    for rel in dirty {
        if rel.is_empty() || crate::sync_yjs::watcher::is_ignored(rel) {
            continue;
        }
        let abs = state.workspace.join(rel);
        match std::fs::read(&abs) {
            Ok(data) => {
                let hash = store.blobs.put(&data)?;
                files.insert(rel.clone(), FileEntry { hash, size: data.len() as u64 });
            }
            Err(_) => {
                files.remove(rel);
            }
        }
    }
    Ok(Snapshot::new(files))
}
