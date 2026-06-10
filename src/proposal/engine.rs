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
use super::model::{Proposal, ProposalSource, ProposalStatus};
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
    let device_label = crate::identity::DeviceIdentity::load_or_generate(None)
        .map(|d| d.device_label)
        .unwrap_or_default();

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
                create_proposal(
                    &state, &store, &prev, &disk, diff.changed_paths(), &device_label,
                    ProposalSource::Ambient, ProposalStatus::Pending,
                )?;
                store.set_baseline(&disk.id)?;
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
    let mut interactive_rx = state.interactive_writes.subscribe();
    let mut review_rx = state.review_writes.subscribe();
    let mut dirty: BTreeSet<String> = BTreeSet::new();
    // Paths written by interactive surfaces (browser editor, P2P CRDT sync,
    // UI file operations). These are live edits the user already saw happen —
    // they become auto-accepted proposals (history + revert, no review).
    // Interactive membership wins over watcher dirtiness because UI file
    // operations trigger both.
    let mut interactive: BTreeSet<String> = BTreeSet::new();
    let mut rescan = false;
    // Paths the review API restored (reject/revert), mapped to the exact blob
    // hash the restoration wrote (None = path deleted). Changes that land on
    // the announced content fold into the baseline without a proposal —
    // otherwise every review decision would spawn a follow-up proposal.
    let mut expected: BTreeMap<String, Option<String>> = BTreeMap::new();

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
            path = interactive_rx.recv() => match path {
                Ok(path) => { interactive.insert(path); }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    // Unknown interactive writes were dropped; a rescan will
                    // surface them as pending proposals — noisy but never lossy.
                    tracing::warn!("[proposal] interactive stream lagged by {n}");
                    rescan = true;
                    dirty.insert(String::new());
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            announced = review_rx.recv() => match announced {
                Ok((path, hash)) => { expected.insert(path, hash); }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("[proposal] review stream lagged by {n}");
                    rescan = true;
                    dirty.insert(String::new());
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            // Re-armed on every loop iteration, so this fires only after
            // IDLE_WINDOW of event silence — a debounce since the last event.
            _ = tokio::time::sleep(IDLE_WINDOW), if !dirty.is_empty() || !interactive.is_empty() => {
                let touched: BTreeSet<String> = dirty.union(&interactive).cloned().collect();
                let result = if rescan {
                    snapshot_workspace(&state, &store)?
                } else {
                    snapshot_dirty(&state, &store, &baseline, &touched)?
                };
                rescan = false;
                dirty.clear();

                let diff = SnapshotDiff::between(&baseline, &result);
                if diff.is_empty() {
                    expected.clear();
                    interactive.clear();
                    continue;
                }

                // Three buckets per changed path:
                //   review restoration landing on its announced content -> fold
                //   interactive live edit  -> auto-accepted proposal
                //   anything else          -> pending proposal for review
                let mut folded = 0usize;
                let mut interactive_paths: Vec<String> = Vec::new();
                let mut agent_paths: Vec<String> = Vec::new();
                for path in diff.changed_paths() {
                    let restored = expected.get(&path).is_some_and(|want| {
                        result.files.get(&path).map(|e| &e.hash) == want.as_ref()
                    });
                    if restored {
                        folded += 1;
                    } else if interactive.contains(&path) {
                        interactive_paths.push(path);
                    } else {
                        agent_paths.push(path);
                    }
                }
                expected.clear();
                interactive.clear();

                store.save_snapshot(&result)?;
                if !interactive_paths.is_empty() {
                    create_proposal(
                        &state, &store, &baseline, &result, interactive_paths, &device_label,
                        ProposalSource::Interactive, ProposalStatus::Accepted,
                    )?;
                }
                if !agent_paths.is_empty() {
                    create_proposal(
                        &state, &store, &baseline, &result, agent_paths, &device_label,
                        ProposalSource::Ambient, ProposalStatus::Pending,
                    )?;
                }
                if folded > 0 {
                    tracing::info!("[proposal] folded {folded} review-restored paths into baseline");
                }
                store.set_baseline(&result.id)?;
                baseline = result;
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn create_proposal(
    state: &AppState,
    store: &ProposalStore,
    base: &Snapshot,
    result: &Snapshot,
    changed_paths: Vec<String>,
    device_label: &str,
    source: ProposalSource,
    status: ProposalStatus,
) -> anyhow::Result<()> {
    let mut proposal = Proposal::ambient(
        state.circle_id.clone(),
        base.id.clone(),
        result.id.clone(),
        changed_paths,
    );
    proposal.source = source;
    proposal.status = status;
    proposal.origin_peer_id = state.peer_id.clone();
    proposal.origin_device = device_label.to_string();
    store.save_proposal(&proposal)?;
    tracing::info!(
        "[proposal] created {} ({:?}/{:?}, {} paths)",
        proposal.id,
        source,
        status,
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
