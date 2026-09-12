//! Reclaiming proposal history and the blobs it pins.
//!
//! Nothing in the proposal subsystem ever deleted anything. Snapshots, proposal
//! records and content-addressed blobs only accumulated, so a Circle's storage
//! grew without bound for as long as it was used. One real Circle reached 1.6 GB
//! of blobs — 3575 of them, the largest 140 MB — against 39 proposals.
//!
//! Collection is mark-and-sweep over two independent questions:
//!
//! 1. **Which proposals are still worth keeping?** Decided ones age out;
//!    undecided ones never do, however old, because dropping a proposal a
//!    reviewer has not seen loses their work.
//! 2. **Which blobs are still reachable?** Everything named by a snapshot that
//!    a surviving proposal — or the baseline — refers to.
//!
//! Two safety rules matter more than the reclaim:
//!
//! * **A young blob is never swept.** A blob is written before the snapshot
//!   that names it is saved, so a blob created moments ago can look unreachable
//!   while its snapshot is still being built. [`MIN_BLOB_AGE`] keeps that race
//!   from eating live content.
//! * **A peer may still need it.** A blob unreferenced here can be the copy a
//!   peer that has not synced is about to ask for over `WantBlobs`. The
//!   retention window is therefore far longer than any plausible absence,
//!   rather than a tight bound tuned to reclaim the most space.

use super::model::ProposalStatus;
use super::store::ProposalStore;
use anyhow::Result;
use std::collections::BTreeSet;

/// How long a decided proposal is kept before it can be collected.
pub const PROPOSAL_RETENTION_DAYS: i64 = 30;

/// A blob younger than this is never swept, whatever reachability says.
pub const MIN_BLOB_AGE: std::time::Duration = std::time::Duration::from_secs(60 * 60);

#[derive(Debug, Default, PartialEq, Eq)]
pub struct GcReport {
    pub proposals_removed: usize,
    pub snapshots_removed: usize,
    pub blobs_removed: usize,
    pub bytes_reclaimed: u64,
}

impl GcReport {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Whether a proposal may be collected once it is old enough.
///
/// `Pending` and `Conflicted` are excluded on purpose: both are waiting on a
/// person. Age is not evidence that a decision was made.
fn is_decided(status: ProposalStatus) -> bool {
    matches!(
        status,
        ProposalStatus::Accepted
            | ProposalStatus::Synced
            | ProposalStatus::Rejected
            | ProposalStatus::Reverted
    )
}

/// Collect expired proposals, the snapshots only they referenced, and the blobs
/// only those snapshots referenced.
pub fn collect(store: &ProposalStore) -> Result<GcReport> {
    let cutoff = chrono::Utc::now() - chrono::Duration::days(PROPOSAL_RETENTION_DAYS);
    let mut report = GcReport::default();

    // ── Mark: which proposals survive ────────────────────────────────────────
    let all = store.list_proposals();
    let (expired, kept): (Vec<_>, Vec<_>) = all
        .into_iter()
        .partition(|p| is_decided(p.status) && p.updated_at.unwrap_or(p.created_at) < cutoff);

    // ── Mark: which snapshots are still reachable ────────────────────────────
    let mut live_snapshots: BTreeSet<String> = kept
        .iter()
        .flat_map(|p| [p.base_snapshot.clone(), p.result_snapshot.clone()])
        .collect();
    // The baseline is the workspace's current reference point; collecting it
    // would leave the engine unable to diff anything.
    if let Some(baseline) = store.baseline_id() {
        live_snapshots.insert(baseline);
    }

    // ── Mark: which blobs are still reachable ────────────────────────────────
    let mut live_blobs: BTreeSet<String> = BTreeSet::new();
    for id in &live_snapshots {
        if let Ok(snapshot) = store.load_snapshot(id) {
            for entry in snapshot.files.values() {
                live_blobs.insert(entry.hash.clone());
            }
        }
    }

    // ── Sweep: proposals ─────────────────────────────────────────────────────
    for proposal in &expired {
        if store.delete_proposal(&proposal.id).is_ok() {
            report.proposals_removed += 1;
        }
    }

    // ── Sweep: snapshots no surviving proposal refers to ─────────────────────
    for id in store.list_snapshot_ids() {
        if !live_snapshots.contains(&id) && store.delete_snapshot(&id).is_ok() {
            report.snapshots_removed += 1;
        }
    }

    // ── Sweep: unreachable blobs ─────────────────────────────────────────────
    let now = std::time::SystemTime::now();
    for (hash, size, modified) in store.blobs.list()? {
        if live_blobs.contains(&hash) {
            continue;
        }
        // A blob is written before the snapshot naming it is saved, so one
        // written moments ago may look unreachable while still being live.
        if let Ok(age) = now.duration_since(modified) {
            if age < MIN_BLOB_AGE {
                continue;
            }
        } else {
            // A timestamp in the future means a clock we cannot reason about.
            continue;
        }
        if store.blobs.remove(&hash).is_ok() {
            report.blobs_removed += 1;
            report.bytes_reclaimed += size;
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proposal::model::{Confidence, Proposal, ProposalSource};
    use crate::proposal::snapshot::{FileEntry, Snapshot};
    use std::collections::BTreeMap;

    fn store() -> (tempfile::TempDir, ProposalStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = ProposalStore::open(dir.path()).unwrap();
        (dir, store)
    }

    fn snapshot_with(store: &ProposalStore, files: &[(&str, &[u8])]) -> String {
        let mut map = BTreeMap::new();
        for (path, body) in files {
            let hash = store.blobs.put(body).unwrap();
            map.insert(
                path.to_string(),
                FileEntry {
                    hash,
                    size: body.len() as u64,
                },
            );
        }
        let snap = Snapshot::new(map);
        store.save_snapshot(&snap).unwrap();
        snap.id
    }

    fn proposal(base: &str, result: &str, status: ProposalStatus, age_days: i64) -> Proposal {
        let when = chrono::Utc::now() - chrono::Duration::days(age_days);
        Proposal {
            id: uuid::Uuid::new_v4().to_string(),
            circle_id: "c".into(),
            base_snapshot: base.into(),
            result_snapshot: result.into(),
            changed_paths: vec![],
            status,
            source: ProposalSource::Ambient,
            actor_id: None,
            actor_hint: None,
            confidence: Confidence::Unknown,
            trigger_id: None,
            session_id: None,
            relay_path: Vec::new(),
            origin_peer_id: String::new(),
            origin_device: String::new(),
            created_at: when,
            updated_at: Some(when),
        }
    }

    /// Backdate every blob so the young-blob guard does not mask the result.
    fn age_blobs(store: &ProposalStore) {
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60 * 60 * 24);
        for (hash, _, _) in store.blobs.list().unwrap() {
            let path = store.blobs.path_for(&hash).unwrap();
            let t = filetime::FileTime::from_system_time(old);
            let _ = filetime::set_file_mtime(&path, t);
        }
    }

    #[test]
    fn an_expired_decided_proposal_and_its_blobs_are_reclaimed() {
        let (_d, store) = store();
        let base = snapshot_with(&store, &[("a.txt", b"old content")]);
        let result = snapshot_with(&store, &[("a.txt", b"new content")]);
        store
            .save_proposal(&proposal(&base, &result, ProposalStatus::Accepted, 90))
            .unwrap();
        age_blobs(&store);

        let report = collect(&store).unwrap();
        assert_eq!(report.proposals_removed, 1);
        assert_eq!(report.snapshots_removed, 2);
        assert_eq!(report.blobs_removed, 2);
        assert!(report.bytes_reclaimed > 0);
        assert!(store.blobs.list().unwrap().is_empty());
    }

    #[test]
    fn a_recent_proposal_is_untouched() {
        let (_d, store) = store();
        let base = snapshot_with(&store, &[("a.txt", b"x")]);
        let result = snapshot_with(&store, &[("a.txt", b"y")]);
        store
            .save_proposal(&proposal(&base, &result, ProposalStatus::Accepted, 1))
            .unwrap();
        age_blobs(&store);

        assert_eq!(collect(&store).unwrap(), GcReport::default());
        assert_eq!(store.blobs.list().unwrap().len(), 2);
    }

    /// Age is not evidence that anyone decided. A proposal still waiting on a
    /// person must survive however old it is.
    #[test]
    fn an_undecided_proposal_survives_forever() {
        let (_d, store) = store();
        for status in [ProposalStatus::Pending, ProposalStatus::Conflicted] {
            let base = snapshot_with(&store, &[("a.txt", b"x")]);
            let result = snapshot_with(&store, &[("a.txt", b"y")]);
            store
                .save_proposal(&proposal(&base, &result, status, 3650))
                .unwrap();
        }
        age_blobs(&store);

        let report = collect(&store).unwrap();
        assert_eq!(report.proposals_removed, 0);
        assert_eq!(report.blobs_removed, 0);
        assert_eq!(store.list_proposals().len(), 2);
    }

    /// Content addressing means two proposals can name the same blob. Removing
    /// one must not pull content out from under the other.
    #[test]
    fn a_blob_shared_with_a_live_proposal_is_kept() {
        let (_d, store) = store();
        let shared: &[u8] = b"shared content";
        let old_base = snapshot_with(&store, &[("a.txt", shared)]);
        let old_result = snapshot_with(&store, &[("a.txt", b"changed")]);
        let live_base = snapshot_with(&store, &[("b.txt", shared)]);
        let live_result = snapshot_with(&store, &[("b.txt", b"also changed")]);
        store
            .save_proposal(&proposal(
                &old_base,
                &old_result,
                ProposalStatus::Accepted,
                90,
            ))
            .unwrap();
        store
            .save_proposal(&proposal(
                &live_base,
                &live_result,
                ProposalStatus::Pending,
                1,
            ))
            .unwrap();
        age_blobs(&store);

        collect(&store).unwrap();
        let shared_hash = crate::proposal::blob::BlobStore::hash(shared);
        assert!(
            store.blobs.contains(&shared_hash),
            "a blob still named by a live snapshot must survive"
        );
        assert_eq!(store.blobs.get(&shared_hash).unwrap(), shared);
    }

    /// A blob is written before the snapshot naming it is saved, so a freshly
    /// written one can look unreachable while still being live.
    #[test]
    fn a_young_unreferenced_blob_is_not_swept() {
        let (_d, store) = store();
        let hash = store.blobs.put(b"just written").unwrap();
        // Deliberately not aged.
        let report = collect(&store).unwrap();
        assert_eq!(report.blobs_removed, 0);
        assert!(store.blobs.contains(&hash));
    }

    #[test]
    fn the_baseline_snapshot_is_never_collected() {
        let (_d, store) = store();
        let baseline = snapshot_with(&store, &[("a.txt", b"baseline content")]);
        store.set_baseline(&baseline).unwrap();
        age_blobs(&store);

        let report = collect(&store).unwrap();
        assert_eq!(report.snapshots_removed, 0);
        assert_eq!(report.blobs_removed, 0);
        assert!(store.load_snapshot(&baseline).is_ok());
    }

    #[test]
    fn collecting_an_empty_store_is_a_no_op() {
        let (_d, store) = store();
        assert!(collect(&store).unwrap().is_empty());
    }
}
