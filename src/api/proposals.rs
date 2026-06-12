//! Proposal review API (M14).
//!
//! Proposals are created by the ambient engine (`crate::proposal::engine`);
//! this API lists them, shows per-file diffs, and applies review decisions.

use crate::control::CircleEvent;
use crate::daemon::DaemonState;
use crate::proposal::blob::BlobStore;
use crate::proposal::merge::{reverse_apply, RestoreOutcome};
use crate::proposal::model::{Proposal, ProposalStatus};
use crate::proposal::store::ProposalStore;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Serialize;
use serde_json::json;

fn open_store(daemon: &DaemonState, circle_id: &str) -> Result<(crate::state::AppState, ProposalStore), (StatusCode, Json<serde_json::Value>)> {
    let state = daemon.get(circle_id).ok_or((
        StatusCode::NOT_FOUND,
        Json(json!({"error": "circle not found"})),
    ))?;
    let store = ProposalStore::open(&state.workspace).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("proposal store: {e}")})),
        )
    })?;
    Ok((state, store))
}

pub async fn list_proposals(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
) -> impl IntoResponse {
    match open_store(&daemon, &circle_id) {
        Ok((_, store)) => Json(json!(store.list_proposals())).into_response(),
        Err(e) => e.into_response(),
    }
}

#[derive(Serialize)]
struct FileDiff {
    path: String,
    change: &'static str, // "added" | "removed" | "modified"
    /// UTF-8 content; None when the file is binary, absent on that side, or its
    /// content was not synced to this device.
    before: Option<String>,
    after: Option<String>,
    binary: bool,
    /// True when the manifest references content this device does not hold
    /// (a large blob excluded from the proposal bundle). The change is known
    /// to have happened, but the content cannot be rendered or reverted here.
    not_synced: bool,
}

#[derive(Serialize)]
struct ProposalDetail {
    #[serde(flatten)]
    proposal: Proposal,
    files: Vec<FileDiff>,
}

pub async fn get_proposal(
    State(daemon): State<DaemonState>,
    Path((circle_id, proposal_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let (_, store) = match open_store(&daemon, &circle_id) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let proposal = match store.load_proposal(&proposal_id) {
        Ok(p) => p,
        Err(_) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "proposal not found"})),
            )
                .into_response();
        }
    };
    let (base, result) = match (
        store.load_snapshot(&proposal.base_snapshot),
        store.load_snapshot(&proposal.result_snapshot),
    ) {
        (Ok(b), Ok(r)) => (b, r),
        _ => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "snapshot missing for proposal"})),
            )
                .into_response();
        }
    };

    let mut files = Vec::new();
    for path in &proposal.changed_paths {
        // Presence/absence is read from the manifest, which always has the
        // entry even when the content blob was not synced. This keeps the
        // add/remove/modify classification correct for large files.
        let base_entry = base.files.get(path);
        let result_entry = result.files.get(path);
        let change = match (base_entry.is_some(), result_entry.is_some()) {
            (false, true) => "added",
            (true, false) => "removed",
            _ => "modified",
        };
        let before_bytes = base_entry.and_then(|e| store.blobs.get(&e.hash).ok());
        let after_bytes = result_entry.and_then(|e| store.blobs.get(&e.hash).ok());
        // A manifest entry whose blob is missing = content not synced here.
        let not_synced = (base_entry.is_some() && before_bytes.is_none())
            || (result_entry.is_some() && after_bytes.is_none());
        let before = before_bytes.as_ref().and_then(|b| String::from_utf8(b.clone()).ok());
        let after = after_bytes.as_ref().and_then(|b| String::from_utf8(b.clone()).ok());
        let binary = (before_bytes.is_some() && before.is_none())
            || (after_bytes.is_some() && after.is_none());
        files.push(FileDiff { path: path.clone(), change, before, after, binary, not_synced });
    }

    Json(json!(ProposalDetail { proposal, files })).into_response()
}

async fn set_status(
    daemon: DaemonState,
    circle_id: String,
    proposal_id: String,
    new_status: ProposalStatus,
) -> axum::response::Response {
    let (state, store) = match open_store(&daemon, &circle_id) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let mut proposal = match store.load_proposal(&proposal_id) {
        Ok(p) => p,
        Err(_) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "proposal not found"})),
            )
                .into_response();
        }
    };
    // Valid transitions (Copilot/Cursor-style review semantics):
    //   pending  -> accepted   keep the changes
    //   pending  -> rejected   restore files to their pre-change state
    //   accepted -> reverted   undo a previously accepted change
    let allowed_from = match new_status {
        ProposalStatus::Accepted | ProposalStatus::Rejected => ProposalStatus::Pending,
        ProposalStatus::Reverted => ProposalStatus::Accepted,
        _ => ProposalStatus::Pending,
    };
    if proposal.status != allowed_from {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": format!(
                "cannot move proposal from {:?} to {:?}", proposal.status, new_status
            )})),
        )
            .into_response();
    }

    // Reject and revert reverse-apply this proposal's change — git revert,
    // not git reset. Later edits to the same files are preserved via a
    // line-level three-way merge; genuine overlaps abort with 409 before
    // anything is written.
    if matches!(new_status, ProposalStatus::Rejected | ProposalStatus::Reverted) {
        let (base, result) = match (
            store.load_snapshot(&proposal.base_snapshot),
            store.load_snapshot(&proposal.result_snapshot),
        ) {
            (Ok(b), Ok(r)) => (b, r),
            _ => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": "snapshot missing for proposal"})),
                )
                    .into_response();
            }
        };

        // Resolve a manifest entry to its content. A manifest entry whose blob
        // is absent from this device's store is NOT the same as "no file at this
        // snapshot": it means the content was never synced here (e.g. a large
        // blob excluded from the proposal bundle — see MAX_EMBEDDED_BLOB_BYTES).
        // Reverse-apply with such a `None` would misread it as a delete and
        // could destroy the file, so collect these and abort instead.
        let mut missing: Vec<String> = Vec::new();
        let resolve = |entry: Option<&crate::proposal::snapshot::FileEntry>, path: &str, missing: &mut Vec<String>| -> Option<Vec<u8>> {
            match entry {
                None => None, // legitimately absent at this snapshot
                Some(e) => match store.blobs.get(&e.hash) {
                    Ok(bytes) => Some(bytes),
                    Err(_) => {
                        missing.push(path.to_string());
                        None
                    }
                },
            }
        };

        // Dry-run every path first so a conflict aborts with nothing written.
        let mut writes: Vec<(String, RestoreOutcome)> = Vec::new();
        let mut conflicts: Vec<String> = Vec::new();
        for path in &proposal.changed_paths {
            let base_bytes = resolve(base.files.get(path), path, &mut missing);
            let result_bytes = resolve(result.files.get(path), path, &mut missing);
            let current = std::fs::read(state.workspace.join(path)).ok();
            match reverse_apply(base_bytes.as_deref(), result_bytes.as_deref(), current.as_deref()) {
                RestoreOutcome::Conflict => conflicts.push(path.clone()),
                outcome => writes.push((path.clone(), outcome)),
            }
        }
        if !missing.is_empty() {
            missing.sort();
            missing.dedup();
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({
                    "error": "this proposal's content was not synced to this device (large file); review it where the change originated",
                    "missing": missing,
                })),
            )
                .into_response();
        }
        if !conflicts.is_empty() {
            return (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "later changes overlap this proposal; resolve the files manually, then retry",
                    "conflicts": conflicts,
                })),
            )
                .into_response();
        }

        for (path, outcome) in &writes {
            let abs = state.workspace.join(path);
            match outcome {
                RestoreOutcome::Unchanged => {}
                RestoreOutcome::Write(content) => {
                    if let Some(parent) = abs.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if let Err(e) = std::fs::write(&abs, content) {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({"error": format!("restoring {path}: {e}")})),
                        )
                            .into_response();
                    }
                    // Tell the engine the exact content this path lands on so
                    // the restoration folds into the baseline silently.
                    let _ = state
                        .review_writes
                        .send((path.clone(), Some(BlobStore::hash(content))));
                }
                RestoreOutcome::Delete => {
                    let _ = std::fs::remove_file(&abs);
                    let _ = state.review_writes.send((path.clone(), None));
                }
                RestoreOutcome::Conflict => unreachable!("conflicts abort above"),
            }
        }
    }

    proposal.status = new_status;
    if let Err(e) = store.save_proposal(&proposal) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("saving proposal: {e}")})),
        )
            .into_response();
    }
    // Replicate the decision so every device reflects the new status.
    crate::proposal::sync::publish_proposal(&state, &store, &proposal);
    let status_str = serde_json::to_value(new_status)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    let _ = state.events.send(CircleEvent::ProposalUpdated {
        proposal_id: proposal.id.clone(),
        status: status_str.clone(),
    });
    Json(json!({"status": status_str, "proposal_id": proposal.id})).into_response()
}

pub async fn accept_proposal(
    State(daemon): State<DaemonState>,
    Path((circle_id, proposal_id)): Path<(String, String)>,
) -> impl IntoResponse {
    set_status(daemon, circle_id, proposal_id, ProposalStatus::Accepted).await
}

pub async fn reject_proposal(
    State(daemon): State<DaemonState>,
    Path((circle_id, proposal_id)): Path<(String, String)>,
) -> impl IntoResponse {
    set_status(daemon, circle_id, proposal_id, ProposalStatus::Rejected).await
}

pub async fn revert_proposal(
    State(daemon): State<DaemonState>,
    Path((circle_id, proposal_id)): Path<(String, String)>,
) -> impl IntoResponse {
    set_status(daemon, circle_id, proposal_id, ProposalStatus::Reverted).await
}
