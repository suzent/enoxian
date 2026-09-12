//! Where a Circle keeps its per-workspace state.
//!
//! Three sibling directories — `.enox_crdt`, `.enox_events`, `.enox_proposals` —
//! grew independently and each claimed its own top level in the user's working
//! directory. They are one subsystem's storage, so they live under one root:
//!
//! ```text
//! .enox/
//!   crdt/       per-file CRDT state
//!   events/     append-only workspace event log
//!   proposals/  proposal records, snapshots, blobs
//! ```
//!
//! [`migrate_legacy_layout`] moves an existing workspace over. It runs before
//! any store opens, because a store that creates its directory first would
//! leave the migration with nowhere to move the old one to.

use std::path::{Path, PathBuf};

/// Root for all per-workspace state.
pub const ROOT: &str = ".enox";

pub const CRDT_DIR: &str = "crdt";
pub const EVENTS_DIR: &str = "events";
pub const PROPOSALS_DIR: &str = "proposals";

/// Pre-consolidation directory names, paired with their new home.
const LEGACY: &[(&str, &str)] = &[
    (".enox_crdt", CRDT_DIR),
    (".enox_events", EVENTS_DIR),
    (".enox_proposals", PROPOSALS_DIR),
];

pub fn root(workspace: &Path) -> PathBuf {
    workspace.join(ROOT)
}

pub fn crdt(workspace: &Path) -> PathBuf {
    root(workspace).join(CRDT_DIR)
}

pub fn events(workspace: &Path) -> PathBuf {
    root(workspace).join(EVENTS_DIR)
}

pub fn proposals(workspace: &Path) -> PathBuf {
    root(workspace).join(PROPOSALS_DIR)
}

/// Whether `rel` (workspace-relative, forward slashes) is enoxian's own state,
/// under either layout. Used to keep the daemon's storage out of its own sync.
pub fn is_state_path(rel: &str) -> bool {
    let first = rel.split('/').next().unwrap_or(rel);
    first == ROOT || LEGACY.iter().any(|(old, _)| *old == first)
}

/// Move a pre-consolidation workspace to the new layout.
///
/// Conservative by design, because this runs unattended against a directory the
/// user also keeps their work in:
///
/// * A legacy directory is moved only when its destination does not exist. If
///   both are present the new one wins and the old is left alone rather than
///   merged — silently combining two states could resurrect content the newer
///   one had already dropped.
/// * A failed move is logged and skipped, not retried or forced. The store then
///   starts empty at the new path, which rebuilds, where a half-moved directory
///   might not.
///
/// Returns how many directories were moved.
pub fn migrate_legacy_layout(workspace: &Path) -> usize {
    let mut moved = 0;
    for (legacy_name, new_name) in LEGACY {
        let legacy = workspace.join(legacy_name);
        if !legacy.is_dir() {
            continue;
        }
        let destination = root(workspace).join(new_name);
        if destination.exists() {
            tracing::warn!(
                "[layout] {} and {} both exist; keeping the new one and leaving the old in place",
                legacy.display(),
                destination.display()
            );
            continue;
        }
        if let Some(parent) = destination.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!("[layout] creating {}: {e}", parent.display());
                continue;
            }
        }
        match std::fs::rename(&legacy, &destination) {
            Ok(()) => {
                tracing::info!(
                    "[layout] moved {} to {}",
                    legacy.display(),
                    destination.display()
                );
                moved += 1;
            }
            Err(e) => tracing::warn!(
                "[layout] could not move {} to {}: {e}",
                legacy.display(),
                destination.display()
            ),
        }
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }

    #[test]
    fn legacy_directories_move_under_one_root() {
        let dir = tempfile::tempdir().unwrap();
        let w = dir.path();
        touch(&w.join(".enox_crdt/repo/a.bin"));
        touch(&w.join(".enox_events/events/e1.json"));
        touch(&w.join(".enox_proposals/blobs/aa/bb"));

        assert_eq!(migrate_legacy_layout(w), 3);

        assert!(crdt(w).join("repo/a.bin").exists());
        assert!(events(w).join("events/e1.json").exists());
        assert!(proposals(w).join("blobs/aa/bb").exists());
        assert!(!w.join(".enox_crdt").exists());
        assert!(!w.join(".enox_proposals").exists());
    }

    #[test]
    fn migration_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let w = dir.path();
        touch(&w.join(".enox_crdt/a.bin"));

        assert_eq!(migrate_legacy_layout(w), 1);
        // Nothing left to move, and the second pass must not disturb anything.
        assert_eq!(migrate_legacy_layout(w), 0);
        assert!(crdt(w).join("a.bin").exists());
    }

    /// Merging two states could resurrect content the newer one already
    /// dropped, so the new layout wins and the old is left untouched.
    #[test]
    fn an_existing_destination_is_never_merged_into() {
        let dir = tempfile::tempdir().unwrap();
        let w = dir.path();
        touch(&w.join(".enox_crdt/old.bin"));
        touch(&crdt(w).join("new.bin"));

        assert_eq!(migrate_legacy_layout(w), 0);
        assert!(crdt(w).join("new.bin").exists());
        assert!(!crdt(w).join("old.bin").exists());
        // The old directory survives, so nothing is lost and it can be
        // inspected by hand.
        assert!(w.join(".enox_crdt/old.bin").exists());
    }

    #[test]
    fn a_clean_workspace_needs_no_migration() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(migrate_legacy_layout(dir.path()), 0);
    }

    #[test]
    fn state_paths_are_recognised_under_either_layout() {
        assert!(is_state_path(".enox/crdt/a.bin"));
        assert!(is_state_path(".enox"));
        assert!(is_state_path(".enox_proposals/blobs/aa/bb"));
        assert!(is_state_path(".enox_events/events/e1.json"));
        // A user file that merely starts with the same letters is not state.
        assert!(!is_state_path(".enoxious/notes.txt"));
        assert!(!is_state_path("src/main.rs"));
    }
}
