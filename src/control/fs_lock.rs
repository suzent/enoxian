use anyhow::Result;
use std::path::Path;

/// Set a file read-only (lock) or writable (unlock).
/// Uses std::fs::Permissions which maps to:
///   Unix  — chmod 444 / 644
///   Windows — SetFileAttributes FILE_ATTRIBUTE_READONLY
pub async fn set_readonly(path: &Path, readonly: bool) -> Result<()> {
    if !path.exists() {
        return Ok(()); // nothing to protect yet
    }
    let mut perms = tokio::fs::metadata(path).await?.permissions();
    perms.set_readonly(readonly);
    tokio::fs::set_permissions(path, perms).await?;
    Ok(())
}
