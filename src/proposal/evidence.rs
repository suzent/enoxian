//! Native ACP write evidence. No file content passes through the coordination CLI.
use super::{
    runs::atomic_json,
    session::LocalChangeSession,
    snapshot::{FileEntry, Snapshot},
    store::ProposalStore,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

// Orders cooperating client writes and evidence capture on this device. Shell
// tools and other programs do not take this lock and cannot claim attribution.
pub static WRITE_ORDER: Mutex<()> = Mutex::new(());
#[derive(Clone, Serialize, Deserialize)]
pub struct WriteEvidence {
    pub id: String,
    pub checkpoint: String,
    pub session: LocalChangeSession,
    pub path: String,
    pub before: Snapshot,
    pub after: Snapshot,
    pub completed: bool,
}

pub fn write(
    workspace: &Path,
    circle_dir: &Path,
    session: &LocalChangeSession,
    abs: &Path,
    content: &[u8],
) -> Result<()> {
    let _order = WRITE_ORDER
        .lock()
        .map_err(|_| anyhow::anyhow!("write journal poisoned"))?;
    write_ordered(workspace, circle_dir, session, abs, content)
}

pub(crate) fn file_order(workspace: &Path) -> Result<std::fs::File> {
    let root = crate::store::layout::proposals(workspace);
    std::fs::create_dir_all(&root)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("write.lock"))?;
    file.lock()?;
    Ok(file)
}

pub(crate) fn write_ordered(
    workspace: &Path,
    circle_dir: &Path,
    session: &LocalChangeSession,
    abs: &Path,
    content: &[u8],
) -> Result<()> {
    let _file_order = file_order(workspace)?;
    let rel = abs
        .strip_prefix(workspace)?
        .to_string_lossy()
        .replace('\\', "/");
    super::validate_workspace_path(&rel)?;
    let store = ProposalStore::open(workspace)?;
    let mut before = Snapshot::new(Default::default());
    match std::fs::read(abs) {
        Ok(bytes) => {
            before.files.insert(
                rel.clone(),
                FileEntry {
                    hash: store.blobs.put(&bytes)?,
                    size: bytes.len() as u64,
                },
            );
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let mut after = Snapshot::new(Default::default());
    after.files.insert(
        rel.clone(),
        FileEntry {
            hash: store.blobs.put(content)?,
            size: content.len() as u64,
        },
    );
    let mut evidence = WriteEvidence {
        id: uuid::Uuid::new_v4().to_string(),
        checkpoint: uuid::Uuid::new_v4().to_string(),
        session: session.clone(),
        path: rel,
        before,
        after,
        completed: false,
    };
    let record = circle_dir
        .join("write_evidence")
        .join(format!("{}.json", evidence.id));
    atomic_json(&record, &evidence)?;
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(abs, content).with_context(|| format!("writing {}", abs.display()))?;
    evidence.completed = true;
    atomic_json(&record, &evidence)?;
    Ok(())
}

pub fn pending(dir: &Path) -> Result<Vec<(PathBuf, WriteEvidence)>> {
    let root = dir.join("write_evidence");
    if !root.exists() {
        return Ok(vec![]);
    }
    let mut out = vec![];
    for entry in std::fs::read_dir(root)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "json") {
            let evidence: WriteEvidence = serde_json::from_slice(&std::fs::read(&path)?)?;
            if evidence.completed {
                out.push((path, evidence));
            }
        }
    }
    out.sort_by(|a, b| {
        a.1.after
            .created_at
            .cmp(&b.1.after.created_at)
            .then(a.1.id.cmp(&b.1.id))
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proposal::session::SessionMode;
    #[test]
    fn overlapping_writes_keep_each_operation_and_actor() {
        let w = tempfile::tempdir().unwrap();
        let d = tempfile::tempdir().unwrap();
        let path = w.path().join("same.txt");
        std::fs::write(&path, "human").unwrap();
        for agent in ["a", "b"] {
            let mut s =
                LocalChangeSession::start("c".into(), "base".into(), SessionMode::ManagedProcess);
            s.actor_id = Some(agent.into());
            write(w.path(), d.path(), &s, &path, agent.as_bytes()).unwrap();
        }
        let records = pending(d.path()).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].1.after.files, records[1].1.before.files);
        assert_eq!(records[0].1.session.actor_id.as_deref(), Some("a"));
        assert_eq!(records[1].1.session.actor_id.as_deref(), Some("b"));
    }
}
