//! Snapshot manifests: a point-in-time map of workspace paths to blob hashes.
//!
//! A snapshot does not contain file content; content lives in the blob store
//! (`super::blob`). Proposals reference a base snapshot (S0) and a result
//! snapshot (S1).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// SHA-256 of the file content, as stored in the blob store.
    pub hash: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Workspace-relative path (forward slashes) -> file entry.
    pub files: BTreeMap<String, FileEntry>,
}

impl Snapshot {
    pub fn new(files: BTreeMap<String, FileEntry>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            created_at: chrono::Utc::now(),
            files,
        }
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!("{}.json", self.id));
        std::fs::write(&path, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("writing snapshot {}", path.display()))
    }

    pub fn load(dir: &Path, id: &str) -> Result<Self> {
        let path = dir.join(format!("{id}.json"));
        let bytes = std::fs::read(&path)
            .with_context(|| format!("reading snapshot {}", path.display()))?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut files = BTreeMap::new();
        files.insert(
            "src/main.rs".to_string(),
            FileEntry { hash: "ab".repeat(32), size: 42 },
        );
        let snap = Snapshot::new(files);
        snap.save(dir.path()).unwrap();
        let loaded = Snapshot::load(dir.path(), &snap.id).unwrap();
        assert_eq!(loaded.id, snap.id);
        assert_eq!(loaded.files, snap.files);
    }
}
