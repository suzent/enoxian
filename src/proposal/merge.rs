//! Three-way merge of proposals against the canonical workspace.
//!
//! ```text
//! base   = snapshot when the local change session started (S0)
//! main   = latest accepted canonical snapshot
//! result = the proposal's dirty result snapshot (S1)
//! ```
//!
//! Agent edits are commit-level changes, not live CRDT operations.

use super::snapshot::Snapshot;
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    /// No path was changed by both sides; the proposal applies cleanly.
    Clean,
    /// Both sides changed these paths to different content.
    Conflicted { paths: Vec<String> },
}

/// Whole-file three-way merge: a path conflicts when both `main` and `result`
/// changed it relative to `base` and disagree on the outcome. Content-level
/// merging within a file is an M16 adapter concern.
pub fn three_way(base: &Snapshot, main: &Snapshot, result: &Snapshot) -> MergeOutcome {
    let paths: BTreeSet<&String> = base
        .files
        .keys()
        .chain(main.files.keys())
        .chain(result.files.keys())
        .collect();

    let mut conflicts = Vec::new();
    for path in paths {
        let b = base.files.get(path).map(|f| &f.hash);
        let m = main.files.get(path).map(|f| &f.hash);
        let r = result.files.get(path).map(|f| &f.hash);
        if m != b && r != b && m != r {
            conflicts.push(path.clone());
        }
    }

    if conflicts.is_empty() {
        MergeOutcome::Clean
    } else {
        MergeOutcome::Conflicted { paths: conflicts }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proposal::blob::BlobStore;
    use crate::proposal::snapshot::FileEntry;
    use std::collections::BTreeMap;

    fn snap(entries: &[(&str, &str)]) -> Snapshot {
        let files: BTreeMap<String, FileEntry> = entries
            .iter()
            .map(|(path, content)| {
                (
                    path.to_string(),
                    FileEntry {
                        hash: BlobStore::hash(content.as_bytes()),
                        size: content.len() as u64,
                    },
                )
            })
            .collect();
        Snapshot::new(files)
    }

    #[test]
    fn disjoint_edits_merge_clean() {
        let base = snap(&[("a.txt", "one"), ("b.txt", "two")]);
        let main = snap(&[("a.txt", "main edit"), ("b.txt", "two")]);
        let result = snap(&[("a.txt", "one"), ("b.txt", "agent edit")]);
        assert_eq!(three_way(&base, &main, &result), MergeOutcome::Clean);
    }

    #[test]
    fn same_path_different_content_conflicts() {
        let base = snap(&[("a.txt", "one")]);
        let main = snap(&[("a.txt", "main edit")]);
        let result = snap(&[("a.txt", "agent edit")]);
        assert_eq!(
            three_way(&base, &main, &result),
            MergeOutcome::Conflicted { paths: vec!["a.txt".to_string()] }
        );
    }

    #[test]
    fn convergent_edits_merge_clean() {
        let base = snap(&[("a.txt", "one")]);
        let main = snap(&[("a.txt", "same edit")]);
        let result = snap(&[("a.txt", "same edit")]);
        assert_eq!(three_way(&base, &main, &result), MergeOutcome::Clean);
    }

    #[test]
    fn delete_vs_edit_conflicts() {
        let base = snap(&[("a.txt", "one")]);
        let main = snap(&[]);
        let result = snap(&[("a.txt", "agent edit")]);
        assert_eq!(
            three_way(&base, &main, &result),
            MergeOutcome::Conflicted { paths: vec!["a.txt".to_string()] }
        );
    }
}
