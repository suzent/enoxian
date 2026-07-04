//! Binary / hash-only adapter. For non-text content, we can't show a meaningful
//! line diff; report the before/after hashes and sizes so review still conveys
//! "this binary changed, here's the fingerprint."

use super::super::adapters::{DiffEntry, DiffKind, FileChange, ObjChange};
use crate::proposal::blob::BlobStore;

pub fn diff(before: &[u8], after: &[u8]) -> FileChange {
    let before_h = BlobStore::hash(before);
    let after_h = BlobStore::hash(after);
    let entries = vec![DiffEntry::ObjectPath {
        path: "<binary>".to_string(),
        change: ObjChange::Changed,
        before: Some(format!("{} ({} bytes)", short(&before_h), before.len())),
        after: Some(format!("{} ({} bytes)", short(&after_h), after.len())),
    }];
    FileChange {
        kind: DiffKind::Binary,
        entries,
        // A binary file is never "formatting noise".
        formatting_only: false,
    }
}

fn short(hash: &str) -> String {
    hash.chars().take(12).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_hashes_and_sizes() {
        let c = diff(&[0, 1, 2], &[0, 1, 2, 3]);
        assert_eq!(c.kind, DiffKind::Binary);
        assert_eq!(c.entries.len(), 1);
        assert!(!c.formatting_only);
    }
}
