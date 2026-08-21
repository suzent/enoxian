//! Proposal replication bundle.
//!
//! Proposals are local artifacts (records under `<workspace>/.enox_proposals/`),
//! but the review history must be identical on every device. A [`ProposalBundle`]
//! packages everything a remote device needs to store and render a proposal it
//! never observed locally: the proposal record, its base/result snapshot
//! manifests, and the content blobs those manifests reference.
//!
//! Bundles are transferred by the proposal pull protocol
//! (`crate::network::proposal_sync`): on each connection peers exchange the ids
//! they hold, then request and stream the bundles they lack. The disk store is
//! the source of truth; this type is the transfer unit.

use super::model::{Proposal, ProposalStatus};
use super::snapshot::Snapshot;
use super::store::ProposalStore;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Blobs at or below this size are base64-embedded in the bundle. Larger files
/// ship as manifest metadata only (hash + size, recorded in the snapshot);
/// their content is fetched by the proposal pull protocol's blob round. Until a
/// peer has fetched the blob, the diff view shows a placeholder and
/// reject/revert fails cleanly on that device rather than corrupting the file.
///
/// 256 KB comfortably covers source, config, and prose; it excludes images,
/// archives, and built artifacts — exactly the things that should move through
/// the on-demand blob path.
pub const MAX_EMBEDDED_BLOB_BYTES: usize = 256 * 1024;

/// A self-contained, replicable proposal: the record plus everything needed to
/// reconstruct it in another device's [`ProposalStore`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalBundle {
    pub proposal: Proposal,
    pub base_snapshot: Snapshot,
    pub result_snapshot: Snapshot,
    /// hash -> base64(content) for every blob referenced by either snapshot
    /// across the proposal's changed paths. Base64 keeps the bundle JSON-safe;
    /// binary file contents survive the round-trip.
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
                    // Size comes from the manifest, so we can skip oversized
                    // blobs without even reading them off disk. The manifest
                    // entry still travels in the snapshot, so the receiver knows
                    // the path changed and how big it is — only the content is
                    // withheld.
                    if entry.size as usize > MAX_EMBEDDED_BLOB_BYTES {
                        continue;
                    }
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
    /// content-addressed blobs.
    ///
    /// Status divergence is resolved by `incoming_status_wins`: if a local
    /// record exists and already wins (a higher-ranked or newer decision), the
    /// inbound record does NOT overwrite it — so pulling an older `pending` from
    /// a peer can't undo a local `accepted`. Returns true if the local store
    /// changed (new proposal, or a winning inbound status), i.e. the caller
    /// should fire an event / refresh UI.
    pub fn apply_to_store(&self, store: &ProposalStore) -> Result<bool> {
        super::validate_bundle_for_circle(self, &self.proposal.circle_id)?;
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

        match store.load_proposal(&self.proposal.id) {
            Ok(local) => {
                if super::model::incoming_status_wins(&local, &self.proposal) {
                    store.save_proposal(&self.proposal)?;
                    Ok(true)
                } else {
                    // Local record is at least as authoritative — keep it.
                    Ok(false)
                }
            }
            Err(_) => {
                // First time we've seen this proposal.
                store.save_proposal(&self.proposal)?;
                Ok(true)
            }
        }
    }

    pub fn status(&self) -> ProposalStatus {
        self.proposal.status
    }
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
        files.insert(
            path.to_string(),
            FileEntry {
                hash,
                size: content.len() as u64,
            },
        );
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
    fn oversized_blobs_are_excluded_from_the_bundle() {
        let src_dir = tempfile::tempdir().unwrap();
        let src = ProposalStore::open(src_dir.path()).unwrap();
        // base: small file. result: a blob just over the embed cap.
        let base = snap_with(&src, "big.bin", b"small");
        let big = vec![0u8; MAX_EMBEDDED_BLOB_BYTES + 1];
        let result = snap_with(&src, "big.bin", &big);
        let proposal = Proposal::ambient(
            "c".into(),
            base.id.clone(),
            result.id.clone(),
            vec!["big.bin".into()],
        );
        src.save_proposal(&proposal).unwrap();

        let bundle = ProposalBundle::from_store(&src, &proposal).unwrap();
        // The small base blob is embedded; the oversized result blob is not.
        let base_hash = &bundle.base_snapshot.files["big.bin"].hash;
        let result_hash = &bundle.result_snapshot.files["big.bin"].hash;
        assert!(bundle.blobs.contains_key(base_hash), "small blob embedded");
        assert!(
            !bundle.blobs.contains_key(result_hash),
            "large blob excluded"
        );

        // Applied to a fresh store, the manifest entry exists but the content
        // is absent — exactly the "known change, unrenderable content" state.
        let dst_dir = tempfile::tempdir().unwrap();
        let dst = ProposalStore::open(dst_dir.path()).unwrap();
        bundle.apply_to_store(&dst).unwrap();
        let r = dst.load_snapshot(&result.id).unwrap();
        assert!(r.files.contains_key("big.bin"), "manifest entry present");
        assert!(!dst.blobs.contains(result_hash), "large content not stored");
    }

    #[test]
    fn status_change_is_detected() {
        // Publisher store: the device whose engine created the proposal.
        let src_dir = tempfile::tempdir().unwrap();
        let src = ProposalStore::open(src_dir.path()).unwrap();
        let base = snap_with(&src, "f", b"a");
        let result = snap_with(&src, "f", b"b");
        let mut proposal = Proposal::ambient(
            "c".into(),
            base.id.clone(),
            result.id.clone(),
            vec!["f".into()],
        );
        src.save_proposal(&proposal).unwrap();

        // Receiver store: a second device applying the synced bundles.
        let dst_dir = tempfile::tempdir().unwrap();
        let dst = ProposalStore::open(dst_dir.path()).unwrap();

        let b0 = ProposalBundle::from_store(&src, &proposal).unwrap();
        assert!(b0.apply_to_store(&dst).unwrap(), "first delivery is new");
        assert!(!b0.apply_to_store(&dst).unwrap(), "redelivery is a no-op");

        proposal.set_status(ProposalStatus::Rejected);
        src.save_proposal(&proposal).unwrap();
        let b1 = ProposalBundle::from_store(&src, &proposal).unwrap();
        assert!(
            b1.apply_to_store(&dst).unwrap(),
            "winning status flip is a change"
        );
    }

    // The conflict rule must not let a lower-ranked inbound status clobber a
    // local terminal decision — e.g. pulling a stale `pending` from a peer that
    // hasn't seen our `accepted` yet.
    #[test]
    fn losing_inbound_status_does_not_overwrite() {
        let src_dir = tempfile::tempdir().unwrap();
        let src = ProposalStore::open(src_dir.path()).unwrap();
        let base = snap_with(&src, "f", b"a");
        let result = snap_with(&src, "f", b"b");
        let mut pending = Proposal::ambient(
            "c".into(),
            base.id.clone(),
            result.id.clone(),
            vec!["f".into()],
        );
        // Exercise the compatibility path for a historical or future staged
        // proposal; new live-workspace proposals default to accepted history.
        pending.status = ProposalStatus::Pending;

        // Local store has already ACCEPTED this proposal.
        let dst_dir = tempfile::tempdir().unwrap();
        let dst = ProposalStore::open(dst_dir.path()).unwrap();
        let mut accepted = pending.clone();
        accepted.set_status(ProposalStatus::Accepted);
        // Materialize snapshots/blobs in dst too so apply_to_store can run.
        ProposalBundle::from_store(&src, &pending)
            .unwrap()
            .apply_to_store(&dst)
            .unwrap();
        dst.save_proposal(&accepted).unwrap();

        // Inbound is the older PENDING version. It must not win.
        let stale = ProposalBundle::from_store(&src, &pending).unwrap();
        assert!(
            !stale.apply_to_store(&dst).unwrap(),
            "stale pending must not overwrite accepted"
        );
        assert_eq!(
            dst.load_proposal(&pending.id).unwrap().status,
            ProposalStatus::Accepted,
            "local accepted decision preserved"
        );
    }
}
