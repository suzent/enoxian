//! Proposal replication bundle.
//!
//! Proposals are local artifacts (records under `<workspace>/.enox_proposals/`),
//! but the review history must be identical on every device. A [`ProposalBundle`]
//! packages everything a remote device needs to store and render a proposal it
//! never observed locally: the proposal record, its base/result snapshot
//! manifests, and the content blobs those manifests reference.
//!
//! Bundles travel inside the `__control__` Yjs map under
//! [`crate::control::PROPOSALS_KEY`], so they replicate to all peers through the
//! existing CRDT sync path — the same mechanism chat and tasks use.

use super::model::{Proposal, ProposalStatus};
use super::snapshot::Snapshot;
use super::store::ProposalStore;
use crate::control::PROPOSALS_KEY;
use crate::state::AppState;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use yrs::{Map, Transact};

/// A self-contained, replicable proposal: the record plus everything needed to
/// reconstruct it in another device's [`ProposalStore`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalBundle {
    pub proposal: Proposal,
    pub base_snapshot: Snapshot,
    pub result_snapshot: Snapshot,
    /// hash -> base64(content) for every blob referenced by either snapshot
    /// across the proposal's changed paths. Base64 keeps the bundle JSON-safe
    /// for the Yjs string map; binary file contents survive the round-trip.
    pub blobs: BTreeMap<String, String>,
}

impl ProposalBundle {
    /// Build a bundle from a proposal already saved in `store`, collecting the
    /// snapshots and the before/after blobs for each changed path.
    pub fn from_store(store: &ProposalStore, proposal: &Proposal) -> Result<Self> {
        let base_snapshot = store.load_snapshot(&proposal.base_snapshot)?;
        let result_snapshot = store.load_snapshot(&proposal.result_snapshot)?;

        let mut blobs = BTreeMap::new();
        for path in &proposal.changed_paths {
            for snap in [&base_snapshot, &result_snapshot] {
                if let Some(entry) = snap.files.get(path) {
                    if !blobs.contains_key(&entry.hash) {
                        if let Ok(bytes) = store.blobs.get(&entry.hash) {
                            blobs.insert(entry.hash.clone(), base64_encode(&bytes));
                        }
                    }
                }
            }
        }

        Ok(Self {
            proposal: proposal.clone(),
            base_snapshot,
            result_snapshot,
            blobs,
        })
    }

    /// Persist this bundle into `store`: blobs first, then snapshots, then the
    /// proposal record. Idempotent — re-applying the same bundle is a no-op for
    /// content-addressed blobs and overwrites the proposal/snapshot files with
    /// identical bytes. Returns true if the proposal was newly written or its
    /// status changed (i.e. the caller should fire an event / refresh UI).
    pub fn apply_to_store(&self, store: &ProposalStore) -> Result<bool> {
        for (hash, b64) in &self.blobs {
            if let Ok(bytes) = base64_decode(b64) {
                // Verify the content hashes to the advertised key before storing,
                // so a malformed bundle can't poison the content-addressed store.
                if super::blob::BlobStore::hash(&bytes) == *hash {
                    store.blobs.put(&bytes)?;
                }
            }
        }
        store.save_snapshot(&self.base_snapshot)?;
        store.save_snapshot(&self.result_snapshot)?;

        let prev_status = store.load_proposal(&self.proposal.id).ok().map(|p| p.status);
        let changed = prev_status != Some(self.proposal.status);
        store.save_proposal(&self.proposal)?;
        Ok(changed)
    }

    pub fn status(&self) -> ProposalStatus {
        self.proposal.status
    }
}

/// Publish a proposal to the shared control doc so it replicates to every peer.
/// Builds the bundle from `store`, then writes it under `PROPOSALS_KEY[id]`.
/// Local writes (no "p2p" txn origin) are forwarded to peers by the control
/// doc observer wired up in `AppState::new`.
pub fn publish_proposal(state: &AppState, store: &ProposalStore, proposal: &Proposal) {
    let bundle = match ProposalBundle::from_store(store, proposal) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("[proposal] cannot build sync bundle for {}: {e}", proposal.id);
            return;
        }
    };
    let json = match serde_json::to_string(&bundle) {
        Ok(j) => j,
        Err(e) => {
            tracing::warn!("[proposal] cannot serialize bundle for {}: {e}", proposal.id);
            return;
        }
    };
    let map = state.control.get_or_insert_map(PROPOSALS_KEY);
    let mut txn = state.control.transact_mut();
    map.insert(&mut txn, proposal.id.as_str(), json.as_str());
}

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn base64_decode(s: &str) -> Result<Vec<u8>> {
    use base64::Engine;
    Ok(base64::engine::general_purpose::STANDARD.decode(s)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proposal::snapshot::FileEntry;

    fn snap_with(store: &ProposalStore, path: &str, content: &[u8]) -> Snapshot {
        let hash = store.blobs.put(content).unwrap();
        let mut files = BTreeMap::new();
        files.insert(path.to_string(), FileEntry { hash, size: content.len() as u64 });
        let snap = Snapshot::new(files);
        store.save_snapshot(&snap).unwrap();
        snap
    }

    #[test]
    fn bundle_roundtrips_through_a_second_store() {
        let src_dir = tempfile::tempdir().unwrap();
        let src = ProposalStore::open(src_dir.path()).unwrap();
        let base = snap_with(&src, "hi.txt", b"old");
        let result = snap_with(&src, "hi.txt", b"new");
        let mut proposal = Proposal::ambient(
            "c".into(),
            base.id.clone(),
            result.id.clone(),
            vec!["hi.txt".into()],
        );
        proposal.status = ProposalStatus::Accepted;
        src.save_proposal(&proposal).unwrap();

        let bundle = ProposalBundle::from_store(&src, &proposal).unwrap();

        // A fresh store that never saw the original edits.
        let dst_dir = tempfile::tempdir().unwrap();
        let dst = ProposalStore::open(dst_dir.path()).unwrap();
        let changed = bundle.apply_to_store(&dst).unwrap();
        assert!(changed, "newly written proposal counts as changed");

        // The diff is renderable from the destination store alone.
        let loaded = dst.load_proposal(&proposal.id).unwrap();
        assert_eq!(loaded.status, ProposalStatus::Accepted);
        let b = dst.load_snapshot(&loaded.base_snapshot).unwrap();
        let r = dst.load_snapshot(&loaded.result_snapshot).unwrap();
        let before = dst.blobs.get(&b.files["hi.txt"].hash).unwrap();
        let after = dst.blobs.get(&r.files["hi.txt"].hash).unwrap();
        assert_eq!(before, b"old");
        assert_eq!(after, b"new");

        // Re-applying the identical bundle reports no change.
        assert!(!bundle.apply_to_store(&dst).unwrap());
    }

    #[test]
    fn status_change_is_detected() {
        // Publisher store: the device whose engine created the proposal.
        let src_dir = tempfile::tempdir().unwrap();
        let src = ProposalStore::open(src_dir.path()).unwrap();
        let base = snap_with(&src, "f", b"a");
        let result = snap_with(&src, "f", b"b");
        let mut proposal =
            Proposal::ambient("c".into(), base.id.clone(), result.id.clone(), vec!["f".into()]);
        src.save_proposal(&proposal).unwrap();

        // Receiver store: a second device applying the synced bundles.
        let dst_dir = tempfile::tempdir().unwrap();
        let dst = ProposalStore::open(dst_dir.path()).unwrap();

        let b0 = ProposalBundle::from_store(&src, &proposal).unwrap();
        assert!(b0.apply_to_store(&dst).unwrap(), "first delivery is new");
        assert!(!b0.apply_to_store(&dst).unwrap(), "redelivery is a no-op");

        proposal.status = ProposalStatus::Rejected;
        src.save_proposal(&proposal).unwrap();
        let b1 = ProposalBundle::from_store(&src, &proposal).unwrap();
        assert!(b1.apply_to_store(&dst).unwrap(), "status flip is a change");
    }
}
