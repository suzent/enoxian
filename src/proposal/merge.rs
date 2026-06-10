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

/// Outcome of reverse-applying one proposal's change to a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreOutcome {
    /// The path already matches the proposal's base — nothing to do.
    Unchanged,
    /// Write this content to the path.
    Write(Vec<u8>),
    /// Delete the path (it did not exist at the proposal's base and the
    /// reverse-apply leaves it empty).
    Delete,
    /// Later changes overlap this proposal's change and cannot be separated
    /// automatically.
    Conflict,
}

/// Reverse-applies a proposal's change to a single path — git revert, not
/// git reset. When the file has changed since the proposal (e.g. a later
/// proposal added more lines), a line-level three-way merge removes only
/// this proposal's change and keeps the rest:
///
/// ```text
/// ancestor = proposal result (what this proposal produced)
/// ours     = current on-disk content (may contain later edits)
/// theirs   = proposal base (the state this proposal is being undone to)
/// ```
pub fn reverse_apply(
    base: Option<&[u8]>,
    result: Option<&[u8]>,
    current: Option<&[u8]>,
) -> RestoreOutcome {
    if current == base {
        return RestoreOutcome::Unchanged;
    }
    // Fast path: nothing changed since this proposal — plain restore.
    if current == result {
        return match base {
            Some(b) => RestoreOutcome::Write(b.to_vec()),
            None => RestoreOutcome::Delete,
        };
    }
    // Diverged: line-level three-way merge. Binary content can't be merged.
    let to_str = |b: Option<&[u8]>| -> Option<String> {
        match b {
            None => Some(String::new()),
            Some(bytes) => String::from_utf8(bytes.to_vec()).ok(),
        }
    };
    let (Some(result_str), Some(current_str), Some(base_str)) =
        (to_str(result), to_str(current), to_str(base))
    else {
        return RestoreOutcome::Conflict;
    };
    // The reverse patch undoes exactly what this proposal changed; applying
    // it to the current content keeps every line the proposal didn't touch.
    let patch = diffy::create_patch(&result_str, &base_str);
    match diffy::apply(&current_str, &patch) {
        Ok(merged) => {
            if merged.is_empty() && base.is_none() {
                RestoreOutcome::Delete
            } else {
                RestoreOutcome::Write(merged.into_bytes())
            }
        }
        Err(_) => RestoreOutcome::Conflict,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proposal::blob::BlobStore;
    use crate::proposal::snapshot::FileEntry;
    use std::collections::BTreeMap;

    #[test]
    fn reverse_apply_keeps_later_additions() {
        // P1 added "hey"; a later accepted change added "you".
        // Rejecting P1 must remove "hey" but keep "you".
        let base = b"";
        let result = b"hey\n";
        let current = b"hey\nyou\n";
        assert_eq!(
            reverse_apply(Some(base), Some(result), Some(current)),
            RestoreOutcome::Write(b"you\n".to_vec())
        );
    }

    #[test]
    fn reverse_apply_fast_path_when_unchanged_since() {
        assert_eq!(
            reverse_apply(Some(b"old\n"), Some(b"new\n"), Some(b"new\n")),
            RestoreOutcome::Write(b"old\n".to_vec())
        );
        // Proposal created the file; nothing since -> delete it.
        assert_eq!(
            reverse_apply(None, Some(b"new\n"), Some(b"new\n")),
            RestoreOutcome::Delete
        );
    }

    #[test]
    fn reverse_apply_noop_when_already_at_base() {
        assert_eq!(
            reverse_apply(Some(b"old\n"), Some(b"new\n"), Some(b"old\n")),
            RestoreOutcome::Unchanged
        );
    }

    #[test]
    fn reverse_apply_conflicts_on_overlapping_edits() {
        // Proposal changed the line to "b"; someone later changed it to "c".
        // Undoing to "a" overlaps the later edit.
        assert_eq!(
            reverse_apply(Some(b"a\n"), Some(b"b\n"), Some(b"c\n")),
            RestoreOutcome::Conflict
        );
    }

    #[test]
    fn reverse_apply_binary_divergence_conflicts() {
        let binary = [0xffu8, 0xfe, 0x00, 0x01];
        assert_eq!(
            reverse_apply(Some(b"a"), Some(&binary), Some(b"other")),
            RestoreOutcome::Conflict
        );
    }

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
