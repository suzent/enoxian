//! Local workspace proposal layer (M14).
//!
//! Agents, editors, and scripts mutate the normal workspace; enoxian captures
//! before/after state as snapshots and turns the difference into reviewable
//! proposals. See `docs/concepts/proposals.md`.
//!
//! This layer sits alongside the CRDT sync watcher (`crate::sync_yjs::watcher`),
//! not in place of it: the CRDT watcher serves interactive editing, while this
//! layer treats the same file events as proposal evidence.

pub mod adapters;
pub mod blob;
pub mod diff;
pub mod engine;
pub mod evidence;
pub mod gc;
pub mod journal;
pub mod merge;
pub mod model;
pub mod policy;
pub mod runs;
pub mod session;
pub mod snapshot;
pub mod store;
pub mod sync;

use anyhow::{bail, Result};
use std::path::{Component, Path};

/// Validate metadata that may have arrived from another peer before it is used
/// to read or mutate the local workspace.
pub fn validate_for_circle(model: &model::Proposal, expected_circle_id: &str) -> Result<()> {
    if model.circle_id != expected_circle_id {
        bail!(
            "proposal {} belongs to circle {}, not {}",
            model.id,
            model.circle_id,
            expected_circle_id
        );
    }
    validate_storage_id("proposal", &model.id)?;
    validate_storage_id("base snapshot", &model.base_snapshot)?;
    validate_storage_id("result snapshot", &model.result_snapshot)?;
    for path in &model.changed_paths {
        validate_workspace_path(path)?;
    }
    Ok(())
}

pub fn validate_storage_id(kind: &str, id: &str) -> Result<()> {
    uuid::Uuid::parse_str(id).map_err(|_| anyhow::anyhow!("invalid {kind} id: {id}"))?;
    Ok(())
}

pub fn validate_bundle_for_circle(
    bundle: &sync::ProposalBundle,
    expected_circle_id: &str,
) -> Result<()> {
    validate_for_circle(&bundle.proposal, expected_circle_id)?;
    if bundle.base_snapshot.id != bundle.proposal.base_snapshot
        || bundle.result_snapshot.id != bundle.proposal.result_snapshot
    {
        bail!("proposal snapshot identifiers do not match bundle manifests");
    }
    for snapshot in [&bundle.base_snapshot, &bundle.result_snapshot] {
        for path in snapshot.files.keys() {
            validate_workspace_path(path)?;
        }
    }
    Ok(())
}

/// Proposal paths are wire data. They must remain plain relative workspace
/// paths; accepting `..`, roots, or platform prefixes would let reject/revert
/// touch files outside the Circle workspace.
pub fn validate_workspace_path(raw: &str) -> Result<()> {
    if raw.trim().is_empty() {
        bail!("proposal path is empty");
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        bail!("proposal path must be relative: {raw}");
    }
    let mut saw_normal = false;
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                // Guard every layout, not just the current one: a proposal
                // arriving from a peer on the old layout must not be able to
                // write into our metadata either.
                if !saw_normal && crate::store::layout::is_state_path(&part.to_string_lossy()) {
                    bail!("proposal path targets internal metadata: {raw}");
                }
                saw_normal = true;
            }
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => bail!("proposal path escapes workspace: {raw}"),
        }
    }
    if !saw_normal {
        bail!("proposal path is empty");
    }
    Ok(())
}

#[cfg(test)]
mod validation_tests {
    use super::*;
    use crate::proposal::model::Proposal;

    #[test]
    fn proposal_paths_stay_inside_workspace() {
        assert!(validate_workspace_path("src/main.rs").is_ok());
        assert!(validate_workspace_path("../other-circle/secret.txt").is_err());
        assert!(validate_workspace_path("./src/main.rs").is_err());
        assert!(validate_workspace_path(".enox_proposals/proposals/x.json").is_err());
        assert!(validate_storage_id("snapshot", "../escape").is_err());
    }

    #[test]
    fn proposal_circle_must_match_route_circle() {
        let proposal = Proposal::ambient(
            "circle-a".into(),
            uuid::Uuid::new_v4().to_string(),
            uuid::Uuid::new_v4().to_string(),
            vec!["safe.txt".into()],
        );
        assert!(validate_for_circle(&proposal, "circle-a").is_ok());
        assert!(validate_for_circle(&proposal, "circle-b").is_err());
    }
}

/// Resolve symlink aliases (including a not-yet-created leaf) and keep native
/// hooks and cooperative locks on the same workspace-relative key.
pub fn canonical_workspace_path(workspace: &Path, raw: &Path) -> Result<std::path::PathBuf> {
    let root = workspace.canonicalize()?;
    let candidate = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        root.join(raw)
    };
    let mut existing = candidate.as_path();
    let mut suffix = Vec::new();
    while !existing.exists() {
        // A dangling symlink must not be treated as a new regular file.
        if std::fs::symlink_metadata(existing).is_ok() {
            bail!("dangling path alias");
        }
        suffix.push(
            existing
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("invalid path"))?
                .to_owned(),
        );
        existing = existing
            .parent()
            .ok_or_else(|| anyhow::anyhow!("invalid path"))?;
    }
    let mut resolved = existing.canonicalize()?;
    for part in suffix.into_iter().rev() {
        resolved.push(part);
    }
    let rel = resolved
        .strip_prefix(&root)
        .map_err(|_| anyhow::anyhow!("path escapes workspace"))?;
    validate_workspace_path(&rel.to_string_lossy().replace('\\', "/"))?;
    Ok(resolved)
}

#[cfg(all(test, unix))]
mod path_lock_tests {
    use super::*;
    #[test]
    fn aliases_replacements_and_missing_leaves_use_a_stable_key() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap();
        let file = root.join("file");
        std::fs::write(&file, "one").unwrap();
        std::os::unix::fs::symlink(&file, root.join("alias")).unwrap();
        assert_eq!(
            canonical_workspace_path(&root, Path::new("alias")).unwrap(),
            file
        );
        std::fs::rename(&file, root.join("moved")).unwrap();
        assert_eq!(
            canonical_workspace_path(&root, Path::new("file")).unwrap(),
            file
        );
        assert!(canonical_workspace_path(&root, Path::new("alias")).is_err());
        std::fs::write(&file, "replacement").unwrap();
        assert_eq!(
            canonical_workspace_path(&root, Path::new("file")).unwrap(),
            file
        );
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("outside")).unwrap();
        assert!(canonical_workspace_path(&root, Path::new("outside/new")).is_err());
    }
}
