//! Snapshot journal: captures before-blobs for paths touched during a dirty
//! window.
//!
//! When the workspace is clean at snapshot S0, the journal is empty. The first
//! watcher event for a path records that path's pre-change state; when the
//! idle window closes, the result snapshot S1 is taken and the S0 -> S1 diff
//! becomes a proposal.
//!
//! Note on capture timing: by the time a watcher event fires, the file on disk
//! has usually already changed. The authoritative "before" content for a path
//! is its entry in the last clean snapshot's manifest (already in the blob
//! store). `capture_before` reading from disk is a best-effort fallback for
//! paths that were never snapshotted.

use super::blob::BlobStore;
use anyhow::Result;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub struct SnapshotJournal {
    workspace: PathBuf,
    blobs: BlobStore,
    /// path -> before blob hash; `None` means the path did not exist before.
    before: BTreeMap<String, Option<String>>,
}

impl SnapshotJournal {
    pub fn new(workspace: PathBuf, blobs: BlobStore) -> Self {
        Self {
            workspace,
            blobs,
            before: BTreeMap::new(),
        }
    }

    /// Records the pre-change state of `rel_path` on its first event in the
    /// current dirty window. Later events for the same path are no-ops: the
    /// before state is fixed once captured.
    pub fn capture_before(&mut self, rel_path: &str) -> Result<()> {
        if self.before.contains_key(rel_path) {
            return Ok(());
        }
        let abs = self.workspace.join(rel_path);
        let entry = match std::fs::read(&abs) {
            Ok(data) => Some(self.blobs.put(&data)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        self.before.insert(rel_path.to_string(), entry);
        Ok(())
    }

    /// Paths touched in the current dirty window.
    pub fn touched_paths(&self) -> impl Iterator<Item = &str> {
        self.before.keys().map(String::as_str)
    }

    /// Clears the journal after a proposal has been created from it.
    pub fn reset(&mut self) {
        self.before.clear();
    }

    // TODO(M14): wire into the watcher event stream with a configurable
    // idle-window close that snapshots the result state (S1) and emits a
    // proposal. The journal must reuse the watcher's ignore rules
    // (`crate::sync_yjs::watcher`) so temp/conflict files never enter
    // proposals.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_first_state_only() {
        let dir = tempfile::tempdir().unwrap();
        let blob_dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"v1").unwrap();

        let blobs = BlobStore::open(blob_dir.path()).unwrap();
        let mut journal = SnapshotJournal::new(dir.path().to_path_buf(), blobs);

        journal.capture_before("a.txt").unwrap();
        std::fs::write(dir.path().join("a.txt"), b"v2").unwrap();
        journal.capture_before("a.txt").unwrap();

        let v1_hash = BlobStore::hash(b"v1");
        assert_eq!(
            journal.before.get("a.txt").unwrap().as_deref(),
            Some(v1_hash.as_str())
        );
        assert_eq!(journal.touched_paths().collect::<Vec<_>>(), vec!["a.txt"]);
    }

    #[test]
    fn missing_path_recorded_as_new() {
        let dir = tempfile::tempdir().unwrap();
        let blob_dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(blob_dir.path()).unwrap();
        let mut journal = SnapshotJournal::new(dir.path().to_path_buf(), blobs);

        journal.capture_before("new.txt").unwrap();
        assert_eq!(journal.before.get("new.txt").unwrap(), &None);
    }
}
