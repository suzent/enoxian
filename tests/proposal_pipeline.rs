//! End-to-end test of the M14 proposal pipeline using only the public API:
//!
//! ```text
//! workspace at S0
//!   -> agent-style mutations (edit, delete, add)
//!   -> journal captures before-blobs
//!   -> result snapshot S1
//!   -> diff S0 -> S1
//!   -> ambient proposal
//!   -> three-way merge (clean and conflicted)
//!   -> revert a file from its before-blob
//! ```
//!
//! This is the merge gate for the M14 scaffold stack: if this passes, the
//! pieces compose. Daemon wiring (watcher events, idle window, CLI) is the
//! remaining integration work.

use enoxian::proposal::blob::BlobStore;
use enoxian::proposal::diff::SnapshotDiff;
use enoxian::proposal::journal::SnapshotJournal;
use enoxian::proposal::merge::{three_way, MergeOutcome};
use enoxian::proposal::model::{Confidence, Proposal, ProposalSource, ProposalStatus};
use enoxian::proposal::snapshot::{FileEntry, Snapshot};
use std::collections::BTreeMap;
use std::path::Path;

/// Walks a workspace directory and produces a snapshot manifest, storing
/// every file's content in the blob store. Stands in for the daemon-side
/// "snapshot on clean workspace" step that will wrap this same logic.
fn snapshot_workspace(workspace: &Path, blobs: &BlobStore) -> Snapshot {
    let mut files = BTreeMap::new();
    let mut stack = vec![workspace.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let data = std::fs::read(&path).unwrap();
            let hash = blobs.put(&data).unwrap();
            let rel = path
                .strip_prefix(workspace)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            files.insert(
                rel,
                FileEntry {
                    hash,
                    size: data.len() as u64,
                },
            );
        }
    }
    Snapshot::new(files)
}

#[test]
fn ambient_pipeline_end_to_end() {
    let workspace_dir = tempfile::tempdir().unwrap();
    let store_dir = tempfile::tempdir().unwrap();
    let workspace = workspace_dir.path();
    let blobs = BlobStore::open(store_dir.path().join("blobs")).unwrap();

    // 1. Clean workspace with three files at S0.
    std::fs::write(workspace.join("kept.txt"), b"unchanged content").unwrap();
    std::fs::write(workspace.join("edited.txt"), b"original content").unwrap();
    std::fs::write(workspace.join("deleted.txt"), b"doomed content").unwrap();
    let s0 = snapshot_workspace(workspace, &blobs);
    assert_eq!(s0.files.len(), 3);

    // 2. An "agent" mutates the workspace; the journal captures before-state
    //    for each touched path as its first event arrives.
    let journal_blobs = BlobStore::open(store_dir.path().join("blobs")).unwrap();
    let mut journal = SnapshotJournal::new(workspace.to_path_buf(), journal_blobs);

    journal.capture_before("edited.txt").unwrap();
    std::fs::write(workspace.join("edited.txt"), b"agent rewrote this").unwrap();

    journal.capture_before("deleted.txt").unwrap();
    std::fs::remove_file(workspace.join("deleted.txt")).unwrap();

    journal.capture_before("added.txt").unwrap();
    std::fs::write(workspace.join("added.txt"), b"brand new file").unwrap();

    let touched: Vec<&str> = journal.touched_paths().collect();
    assert_eq!(touched, vec!["added.txt", "deleted.txt", "edited.txt"]);

    // 3. Idle window closes: result snapshot S1 and diff.
    let s1 = snapshot_workspace(workspace, &blobs);
    let diff = SnapshotDiff::between(&s0, &s1);
    assert_eq!(diff.added, vec!["added.txt"]);
    assert_eq!(diff.removed, vec!["deleted.txt"]);
    assert_eq!(diff.modified, vec!["edited.txt"]);

    // 4. The engine records the already-live diff as accepted ambient history.
    let proposal = Proposal::ambient(
        "circle-test".into(),
        s0.id.clone(),
        s1.id.clone(),
        diff.changed_paths(),
    );
    assert_eq!(proposal.status, ProposalStatus::Accepted);
    assert_eq!(proposal.source, ProposalSource::Ambient);
    assert_eq!(proposal.confidence, Confidence::Unknown);
    assert_eq!(
        proposal.changed_paths,
        vec!["added.txt", "deleted.txt", "edited.txt"]
    );

    // 5. Merge: canonical state unchanged since S0 -> clean.
    assert_eq!(three_way(&s0, &s0, &s1), MergeOutcome::Clean);

    // 6. Merge: canonical state concurrently changed the same file -> conflict.
    let mut concurrent = s0.files.clone();
    let concurrent_data = b"someone else edited concurrently";
    concurrent.insert(
        "edited.txt".into(),
        FileEntry {
            hash: blobs.put(concurrent_data).unwrap(),
            size: concurrent_data.len() as u64,
        },
    );
    let main = Snapshot::new(concurrent);
    assert_eq!(
        three_way(&s0, &main, &s1),
        MergeOutcome::Conflicted {
            paths: vec!["edited.txt".into()]
        }
    );

    // 7. Revert: restore the edited file from its S0 blob. This is the undo
    //    path that auto-accept depends on.
    let before_hash = &s0.files["edited.txt"].hash;
    let before_content = blobs.get(before_hash).unwrap();
    std::fs::write(workspace.join("edited.txt"), &before_content).unwrap();
    assert_eq!(
        std::fs::read(workspace.join("edited.txt")).unwrap(),
        b"original content"
    );

    // 8. Snapshot manifests round-trip through persistence.
    let manifest_dir = store_dir.path().join("snapshots");
    s0.save(&manifest_dir).unwrap();
    s1.save(&manifest_dir).unwrap();
    let s0_loaded = Snapshot::load(&manifest_dir, &s0.id).unwrap();
    assert_eq!(s0_loaded.files, s0.files);
}
