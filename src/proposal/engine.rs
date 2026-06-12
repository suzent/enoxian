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
    // Value is the author label: None = local device, Some(label) = remote peer.
    let mut interactive: BTreeMap<String, Option<String>> = BTreeMap::new();
    // Snapshot captured when the first interactive write of a *settled* batch
    // arrives, and held across idle windows until the burst goes quiet.
    // Interactive paths diff against this rather than the rolling baseline, and
    // emission is deferred until a path has been quiet for a full window. The
    // two together collapse a round-trip — e.g. add-line then revert arriving
    // more than one IDLE_WINDOW apart over slow P2P sync — into nothing,
    // instead of one proposal per window. See `settle` logic in the idle arm.
    let mut interactive_baseline: Option<Snapshot> = None;
    // Interactive paths written during the window that is currently open. On
    // each idle fire, paths NOT in this set have been quiet for a full window
    // and are ready to emit; freshly-written paths carry over to coalesce
    // further. Drained into `interactive` membership each window.
    let mut interactive_fresh: BTreeSet<String> = BTreeSet::new();
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
            msg = interactive_rx.recv() => match msg {
                Ok((path, author)) => {
                    if interactive_baseline.is_none() {
                        interactive_baseline = Some(baseline.clone());
                    }
                    interactive.entry(path.clone()).or_insert(author);
                    // Mark written-this-window so the idle arm defers its
                    // emission and keeps coalescing until the path goes quiet.
                    interactive_fresh.insert(path);
                }
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
                let interactive_keys: BTreeSet<String> = interactive.keys().cloned().collect();
                let touched: BTreeSet<String> = dirty.union(&interactive_keys).cloned().collect();
                let result = if rescan {
                    snapshot_workspace(&state, &store)?
                } else {
                    snapshot_dirty(&state, &store, &baseline, &touched)?
                };
                rescan = false;
                dirty.clear();

                // Interactive paths written during the window that just closed
                // are still "in flight" — defer them so a round-trip spanning
                // multiple windows keeps coalescing. Paths quiet for a full
                // window are ready to emit. Fresh marks are consumed here.
                let still_coalescing: BTreeSet<String> =
                    std::mem::take(&mut interactive_fresh)
                        .into_iter()
                        .filter(|p| interactive.contains_key(p))
                        .collect();

                // For ambient (agent) paths diff against the rolling baseline.
                // For interactive paths diff against the baseline captured when
                // the burst began, held across windows — so the net of an
                // add-then-revert is empty no matter how the writes are spread
                // across idle windows.
                let ibas = interactive_baseline
                    .clone()
                    .unwrap_or_else(|| baseline.clone());
                let agent_diff = SnapshotDiff::between(&baseline, &result);
                let interactive_diff = SnapshotDiff::between(&ibas, &result);

                if agent_diff.is_empty() && interactive_diff.is_empty() {
                    expected.clear();
                    interactive.clear();
                    interactive_baseline = None;
                    continue;
                }

                let WindowPlan { folded, interactive_by_author, agent_paths } = classify_window(
                    &agent_diff,
                    &interactive_diff,
                    &result,
                    &interactive,
                    &still_coalescing,
                    &expected,
                );
                expected.clear();
                // Settled interactive paths are done; coalescing ones stay so a
                // later window can fold their round-trip against the same `ibas`.
                interactive.retain(|p, _| still_coalescing.contains(p));
                if still_coalescing.is_empty() {
                    // Burst is fully quiet — start the next one fresh.
                    interactive_baseline = None;
                }

                store.save_snapshot(&result)?;
                for (author, paths) in interactive_by_author {
                    let label = author.as_deref().unwrap_or(&device_label);
                    create_proposal(
                        &state, &store, &ibas, &result, paths, label,
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

/// The decision for one idle window: which paths fold silently, which become
/// interactive proposals (grouped by author), and which become pending agent
/// proposals. Pure given the diffs and state — see `classify_window`.
struct WindowPlan {
    folded: usize,
    interactive_by_author: BTreeMap<Option<String>, Vec<String>>,
    agent_paths: Vec<String>,
}

/// Classify a window's changed paths into fold / interactive / agent buckets.
///
/// Three rules, in order of precedence per path:
///   1. A path still coalescing (written during the window that just closed) is
///      held back entirely — neither emitted nor folded — so a multi-window
///      round-trip keeps accumulating against the same interactive baseline.
///   2. A path whose result matches the content a review restoration announced
///      (`expected`) folds silently — a reject/revert landing where it said it
///      would, not a new proposal.
///   3. Otherwise: interactive paths become auto-accepted proposals grouped by
///      author; agent paths become pending proposals.
///
/// Interactive paths are classified from `interactive_diff` (against the held
/// burst baseline) so an add-then-revert whose net is empty never appears here.
/// Agent paths come from `agent_diff` (against the rolling baseline) and exclude
/// anything already owned by an interactive author.
fn classify_window(
    agent_diff: &SnapshotDiff,
    interactive_diff: &SnapshotDiff,
    result: &Snapshot,
    interactive: &BTreeMap<String, Option<String>>,
    still_coalescing: &BTreeSet<String>,
    expected: &BTreeMap<String, Option<String>>,
) -> WindowPlan {
    let mut folded = 0usize;
    let mut interactive_by_author: BTreeMap<Option<String>, Vec<String>> = BTreeMap::new();
    let mut agent_paths: Vec<String> = Vec::new();

    let is_restored = |path: &str| {
        expected.get(path).is_some_and(|want| {
            result.files.get(path).map(|e| &e.hash) == want.as_ref()
        })
    };

    for path in interactive_diff.changed_paths() {
        if still_coalescing.contains(&path) {
            continue;
        }
        if is_restored(&path) {
            folded += 1;
        } else if let Some(author) = interactive.get(&path) {
            interactive_by_author.entry(author.clone()).or_default().push(path);
        }
    }

    for path in agent_diff.changed_paths() {
        if interactive.contains_key(&path) {
            continue; // owned by the interactive bucket above
        }
        if is_restored(&path) {
            folded += 1;
        } else {
            agent_paths.push(path);
        }
    }

    WindowPlan { folded, interactive_by_author, agent_paths }
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
    // Replicate to every peer so all devices show the same review history.
    super::sync::publish_proposal(state, store, &proposal);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proposal::snapshot::FileEntry;

    /// Build a snapshot from (path, content-hash) pairs. Hashes stand in for
    /// content; classify_window only compares hashes, never reads blobs.
    fn snap(entries: &[(&str, &str)]) -> Snapshot {
        let mut files = BTreeMap::new();
        for (path, hash) in entries {
            files.insert(path.to_string(), FileEntry { hash: (*hash).into(), size: 1 });
        }
        Snapshot::new(files)
    }

    fn interactive(paths: &[(&str, Option<&str>)]) -> BTreeMap<String, Option<String>> {
        paths
            .iter()
            .map(|(p, a)| (p.to_string(), a.map(|s| s.to_string())))
            .collect()
    }

    fn set(paths: &[&str]) -> BTreeSet<String> {
        paths.iter().map(|s| s.to_string()).collect()
    }

    // An interactive edit that has gone quiet emits one proposal for its author.
    #[test]
    fn settled_interactive_edit_emits_for_its_author() {
        let ibas = snap(&[("hi.txt", "v0")]);
        let result = snap(&[("hi.txt", "v1")]);
        let idiff = SnapshotDiff::between(&ibas, &result);
        let adiff = SnapshotDiff::between(&result, &result); // no agent baseline change

        let plan = classify_window(
            &adiff,
            &idiff,
            &result,
            &interactive(&[("hi.txt", Some("macbook"))]),
            &set(&[]), // not coalescing — quiet for a full window
            &BTreeMap::new(),
        );

        assert_eq!(plan.agent_paths.len(), 0);
        assert_eq!(plan.folded, 0);
        let by_author = &plan.interactive_by_author[&Some("macbook".to_string())];
        assert_eq!(by_author, &vec!["hi.txt".to_string()]);
    }

    // The core cross-window fix: a path whose net change against the held burst
    // baseline is zero (add then revert) produces NOTHING — no proposal, no
    // fold-count — because it never appears in the interactive diff.
    #[test]
    fn net_zero_round_trip_emits_nothing() {
        let ibas = snap(&[("hi.txt", "v0")]);
        let result = snap(&[("hi.txt", "v0")]); // reverted back to origin
        let idiff = SnapshotDiff::between(&ibas, &result);
        assert!(idiff.is_empty(), "net change is empty");

        let plan = classify_window(
            &SnapshotDiff::default(),
            &idiff,
            &result,
            &interactive(&[("hi.txt", None)]),
            &set(&[]),
            &BTreeMap::new(),
        );

        assert_eq!(plan.interactive_by_author.len(), 0);
        assert_eq!(plan.agent_paths.len(), 0);
        assert_eq!(plan.folded, 0);
    }

    // A path written during the just-closed window is held back: even though it
    // shows a net change now, it is neither emitted nor folded this window.
    #[test]
    fn coalescing_path_is_held_back() {
        let ibas = snap(&[("hi.txt", "v0")]);
        let result = snap(&[("hi.txt", "v1")]); // changed, but still in flight
        let idiff = SnapshotDiff::between(&ibas, &result);

        let plan = classify_window(
            &SnapshotDiff::default(),
            &idiff,
            &result,
            &interactive(&[("hi.txt", Some("suk"))]),
            &set(&["hi.txt"]), // written this window — defer
            &BTreeMap::new(),
        );

        assert_eq!(plan.interactive_by_author.len(), 0, "deferred, not emitted");
        assert_eq!(plan.agent_paths.len(), 0);
    }

    // Agent (non-interactive) paths become pending proposals via the agent diff.
    #[test]
    fn agent_path_becomes_pending() {
        let baseline = snap(&[("gen.txt", "g0")]);
        let result = snap(&[("gen.txt", "g1")]);
        let adiff = SnapshotDiff::between(&baseline, &result);

        let plan = classify_window(
            &adiff,
            &SnapshotDiff::default(),
            &result,
            &BTreeMap::new(), // no interactive paths
            &set(&[]),
            &BTreeMap::new(),
        );

        assert_eq!(plan.agent_paths, vec!["gen.txt".to_string()]);
        assert_eq!(plan.interactive_by_author.len(), 0);
    }

    // A review restoration landing on its announced content folds silently.
    #[test]
    fn review_restored_path_folds() {
        let ibas = snap(&[("hi.txt", "v0")]);
        let result = snap(&[("hi.txt", "restored")]);
        let idiff = SnapshotDiff::between(&ibas, &result);
        let mut expected = BTreeMap::new();
        expected.insert("hi.txt".to_string(), Some("restored".to_string()));

        let plan = classify_window(
            &SnapshotDiff::default(),
            &idiff,
            &result,
            &interactive(&[("hi.txt", None)]),
            &set(&[]),
            &expected,
        );

        assert_eq!(plan.folded, 1);
        assert_eq!(plan.interactive_by_author.len(), 0);
    }
}
