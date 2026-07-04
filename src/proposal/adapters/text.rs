//! Text line-diff adapter — the universal fallback. Produces unified-style line
//! hunks via `diffy`.

use super::super::adapters::{DiffEntry, DiffKind, FileChange};
use serde::Serialize;

/// A contiguous block of line changes.
#[derive(Debug, Clone, Serialize)]
pub struct LineHunk {
    /// 1-based line in the *before* file where this hunk starts.
    pub before_start: usize,
    /// 1-based line in the *after* file where this hunk starts.
    pub after_start: usize,
    /// Lines removed from `before`.
    pub removed: Vec<String>,
    /// Lines added in `after`.
    pub added: Vec<String>,
}

pub fn diff(before: &str, after: &str) -> FileChange {
    let hunks = line_hunks(before, after);
    let formatting_only = !hunks.is_empty()
        && super::super::adapters::whitespace_normalized_eq(before, after);
    FileChange {
        kind: DiffKind::Text,
        entries: hunks.into_iter().map(DiffEntry::LineHunk).collect(),
        formatting_only,
    }
}

/// Compute line hunks from `diffy`'s patch. Groups consecutive insert/delete
/// lines into hunks with their line offsets.
pub(crate) fn line_hunks(before: &str, after: &str) -> Vec<LineHunk> {
    let patch = diffy::create_patch(before, after);
    let mut hunks = Vec::new();
    for h in patch.hunks() {
        let before_start = h.old_range().start().max(1);
        let after_start = h.new_range().start().max(1);
        let mut removed = Vec::new();
        let mut added = Vec::new();
        for line in h.lines() {
            match line {
                diffy::Line::Delete(s) => removed.push(strip_nl(s)),
                diffy::Line::Insert(s) => added.push(strip_nl(s)),
                diffy::Line::Context(_) => {}
            }
        }
        if !removed.is_empty() || !added.is_empty() {
            hunks.push(LineHunk { before_start, after_start, removed, added });
        }
    }
    hunks
}

fn strip_nl(s: &str) -> String {
    s.strip_suffix('\n').unwrap_or(s).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produces_hunks_for_line_changes() {
        let c = diff("a\nb\nc\n", "a\nB\nc\n");
        assert_eq!(c.kind, DiffKind::Text);
        assert_eq!(c.entries.len(), 1);
        assert!(!c.formatting_only);
    }

    #[test]
    fn flags_whitespace_only_change() {
        let c = diff("a b\n", "a  b\n");
        assert!(c.formatting_only, "whitespace-only change is formatting noise");
    }

    #[test]
    fn identical_has_no_hunks() {
        let c = diff("same\n", "same\n");
        assert!(c.entries.is_empty());
        assert!(!c.formatting_only);
    }
}
