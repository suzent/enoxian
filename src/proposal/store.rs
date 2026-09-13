//! On-disk proposal storage under `<workspace>/.enox_proposals/`.
//!
//! ```text
//! .enox_proposals/
//!   blobs/<aa>/<62 hex>     content-addressed file contents
//!   snapshots/<id>.json     snapshot manifests
//!   proposals/<id>.json     proposal records
//!   baseline                id of the current baseline snapshot (S0)
//! ```
//!
//! The directory is dot-prefixed so the CRDT sync watcher ignores it.

use super::blob::BlobStore;
use super::model::Proposal;
use super::snapshot::Snapshot;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Retained for the pre-consolidation layout; the live path comes from
/// [`crate::store::layout`].
pub const LEGACY_STORE_DIR: &str = ".enox_proposals";

pub struct ProposalStore {
    root: PathBuf,
    pub blobs: BlobStore,
}

impl ProposalStore {
    pub fn open(workspace: &Path) -> Result<Self> {
        let root = crate::store::layout::proposals(workspace);
        let blobs = BlobStore::open(root.join("blobs"))?;
        std::fs::create_dir_all(root.join("snapshots"))?;
        std::fs::create_dir_all(root.join("proposals"))?;
        Ok(Self { root, blobs })
    }

    fn snapshots_dir(&self) -> PathBuf {
        self.root.join("snapshots")
    }

    fn proposals_dir(&self) -> PathBuf {
        self.root.join("proposals")
    }

    pub fn save_snapshot(&self, snapshot: &Snapshot) -> Result<()> {
        snapshot.save(&self.snapshots_dir())
    }

    pub fn load_snapshot(&self, id: &str) -> Result<Snapshot> {
        Snapshot::load(&self.snapshots_dir(), id)
    }

    /// The current baseline snapshot id (S0), if one has been established.
    pub fn baseline_id(&self) -> Option<String> {
        let id = std::fs::read_to_string(self.root.join("baseline")).ok()?;
        let id = id.trim().to_string();
        if id.is_empty() {
            None
        } else {
            Some(id)
        }
    }

    pub fn set_baseline(&self, id: &str) -> Result<()> {
        let path = self.root.join("baseline");
        let tmp = self
            .root
            .join(format!(".baseline-{}", uuid::Uuid::new_v4()));
        std::fs::write(&tmp, id)?;
        std::fs::File::open(&tmp)?.sync_all()?;
        std::fs::rename(tmp, path).context("writing baseline pointer")?;
        #[cfg(unix)]
        std::fs::File::open(&self.root)?.sync_all()?;
        Ok(())
    }

    pub fn save_proposal(&self, proposal: &Proposal) -> Result<()> {
        super::validate_storage_id("proposal", &proposal.id)?;
        let path = self.proposals_dir().join(format!("{}.json", proposal.id));
        super::runs::atomic_json(&path, proposal)
            .with_context(|| format!("writing proposal {}", path.display()))
    }

    pub fn load_proposal(&self, id: &str) -> Result<Proposal> {
        super::validate_storage_id("proposal", id)?;
        let path = self.proposals_dir().join(format!("{id}.json"));
        let bytes =
            std::fs::read(&path).with_context(|| format!("reading proposal {}", path.display()))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// All proposals, newest first.
    /// Ids of every stored snapshot.
    pub fn list_snapshot_ids(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(self.snapshots_dir()) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.strip_suffix(".json").map(str::to_string)
            })
            .collect()
    }

    /// Delete a snapshot manifest. Absent is success.
    pub fn delete_snapshot(&self, id: &str) -> Result<()> {
        remove_if_present(&self.snapshots_dir().join(format!("{id}.json")))
    }

    /// Delete a proposal record. Absent is success.
    pub fn delete_proposal(&self, id: &str) -> Result<()> {
        remove_if_present(&self.proposals_dir().join(format!("{id}.json")))
    }

    pub fn list_proposals(&self) -> Vec<Proposal> {
        let mut proposals: Vec<Proposal> = std::fs::read_dir(self.proposals_dir())
            .map(|rd| {
                rd.filter_map(|entry| {
                    let path = entry.ok()?.path();
                    let bytes = std::fs::read(&path).ok()?;
                    serde_json::from_slice(&bytes).ok()
                })
                .collect()
            })
            .unwrap_or_default();
        proposals.sort_by_key(|p| std::cmp::Reverse(p.created_at));
        proposals
    }
}

/// Remove a file, treating "already gone" as success — the caller wants the
/// path absent, not proof that it did the removing.
fn remove_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proposal_roundtrip_and_listing_order() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProposalStore::open(dir.path()).unwrap();

        let older = Proposal::ambient("c".into(), "s0".into(), "s1".into(), vec!["a.txt".into()]);
        let mut newer =
            Proposal::ambient("c".into(), "s1".into(), "s2".into(), vec!["b.txt".into()]);
        newer.created_at = older.created_at + chrono::Duration::seconds(5);
        store.save_proposal(&older).unwrap();
        store.save_proposal(&newer).unwrap();

        let listed = store.list_proposals();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, newer.id, "newest first");

        let loaded = store.load_proposal(&older.id).unwrap();
        assert_eq!(loaded.changed_paths, vec!["a.txt"]);
    }

    #[test]
    fn baseline_pointer() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProposalStore::open(dir.path()).unwrap();
        assert_eq!(store.baseline_id(), None);
        store.set_baseline("snap-123").unwrap();
        assert_eq!(store.baseline_id(), Some("snap-123".into()));
    }
}
