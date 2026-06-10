//! Content-addressed blob store.
//!
//! Stores file contents keyed by their SHA-256 hash so snapshots can reference
//! identical content once, and reverts can restore any prior file state.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// On-disk layout: `<root>/<first two hex chars>/<remaining 62 hex chars>`.
pub struct BlobStore {
    root: PathBuf,
}

impl BlobStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)
            .with_context(|| format!("creating blob store at {}", root.display()))?;
        Ok(Self { root })
    }

    pub fn hash(data: &[u8]) -> String {
        hex::encode(Sha256::digest(data))
    }

    fn blob_path(&self, hash: &str) -> Result<PathBuf> {
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            anyhow::bail!("invalid blob hash: {hash}");
        }
        Ok(self.root.join(&hash[..2]).join(&hash[2..]))
    }

    /// Stores `data` and returns its hash. Idempotent: existing blobs are
    /// never rewritten.
    pub fn put(&self, data: &[u8]) -> Result<String> {
        let hash = Self::hash(data);
        let path = self.blob_path(&hash)?;
        if !path.exists() {
            let dir = path.parent().expect("blob path has a parent");
            std::fs::create_dir_all(dir)?;
            // Temp file + rename so a crash never leaves a torn blob behind.
            let tmp = dir.join(format!(".tmp-{}", uuid::Uuid::new_v4()));
            std::fs::write(&tmp, data)?;
            std::fs::rename(&tmp, &path)?;
        }
        Ok(hash)
    }

    pub fn get(&self, hash: &str) -> Result<Vec<u8>> {
        let path = self.blob_path(hash)?;
        std::fs::read(&path).with_context(|| format!("reading blob {hash}"))
    }

    pub fn contains(&self, hash: &str) -> bool {
        self.blob_path(hash).map(|p| p.exists()).unwrap_or(false)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::open(dir.path()).unwrap();
        let hash = store.put(b"hello proposal layer").unwrap();
        assert!(store.contains(&hash));
        assert_eq!(store.get(&hash).unwrap(), b"hello proposal layer");
        // Idempotent put returns the same hash.
        assert_eq!(store.put(b"hello proposal layer").unwrap(), hash);
    }

    #[test]
    fn rejects_bad_hash() {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::open(dir.path()).unwrap();
        assert!(store.get("../escape").is_err());
        assert!(!store.contains("short"));
    }
}
