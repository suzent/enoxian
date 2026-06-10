//! Snapshot diff generation: which paths changed between two snapshots.
//!
//! Content-level diffs (line, markdown, JSON, code-aware) are M16 adapters;
//! this layer only classifies paths by comparing manifest hashes.

use super::snapshot::Snapshot;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SnapshotDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub modified: Vec<String>,
}

impl SnapshotDiff {
    pub fn between(base: &Snapshot, result: &Snapshot) -> Self {
        let mut diff = Self::default();
        for (path, entry) in &result.files {
            match base.files.get(path) {
                None => diff.added.push(path.clone()),
                Some(before) if before.hash != entry.hash => diff.modified.push(path.clone()),
                Some(_) => {}
            }
        }
        for path in base.files.keys() {
            if !result.files.contains_key(path) {
                diff.removed.push(path.clone());
            }
        }
        diff
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.modified.is_empty()
    }

    /// All touched paths, sorted, for `Proposal::changed_paths`.
    pub fn changed_paths(&self) -> Vec<String> {
        let mut paths: Vec<String> = self
            .added
            .iter()
            .chain(&self.removed)
            .chain(&self.modified)
            .cloned()
            .collect();
        paths.sort();
        paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proposal::snapshot::FileEntry;
    use std::collections::BTreeMap;

    fn snap(entries: &[(&str, &str)]) -> Snapshot {
        let files: BTreeMap<String, FileEntry> = entries
            .iter()
            .map(|(path, content)| {
                (
                    path.to_string(),
                    FileEntry {
                        hash: crate::proposal::blob::BlobStore::hash(content.as_bytes()),
                        size: content.len() as u64,
                    },
                )
            })
            .collect();
        Snapshot::new(files)
    }

    #[test]
    fn classifies_added_removed_modified() {
        let base = snap(&[("a.txt", "one"), ("b.txt", "two"), ("c.txt", "three")]);
        let result = snap(&[("a.txt", "one"), ("b.txt", "changed"), ("d.txt", "new")]);
        let diff = SnapshotDiff::between(&base, &result);
        assert_eq!(diff.added, vec!["d.txt"]);
        assert_eq!(diff.removed, vec!["c.txt"]);
        assert_eq!(diff.modified, vec!["b.txt"]);
        assert_eq!(diff.changed_paths(), vec!["b.txt", "c.txt", "d.txt"]);
    }

    #[test]
    fn identical_snapshots_are_empty() {
        let base = snap(&[("a.txt", "one")]);
        let result = snap(&[("a.txt", "one")]);
        assert!(SnapshotDiff::between(&base, &result).is_empty());
    }
}
