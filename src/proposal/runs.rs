//! Durable records for independent executions. Conversation memory is separate.
use super::session::LocalChangeSession;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    #[serde(default)]
    pub writes_consumed: bool,
    pub session: LocalChangeSession,
    pub owner_pid: u32,
    pub child_pid: Option<u32>,
    pub interrupted: bool,
}

#[derive(Debug, thiserror::Error)]
#[error("conversation already has an active or surviving turn")]
pub struct ConversationBusy;

/// OS ownership is per agent/Circle, shared by daemon and manual launches.
/// Never unlink lease files: unlinking a locked inode permits a second owner.
pub struct RunLease {
    _file: File,
    path: PathBuf,
    pub record: RunRecord,
}

pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("record has no parent")?;
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".tmp-{}", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(&serde_json::to_vec(value)?)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)?;
        #[cfg(unix)]
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result
}

impl RunLease {
    pub fn acquire(dir: &Path, session: LocalChangeSession) -> Result<Self> {
        let agent = session
            .actor_id
            .as_deref()
            .context("run requires an agent")?;
        let root = dir.join("managed_runs");
        std::fs::create_dir_all(&root)?;
        let key = super::blob::BlobStore::hash(agent.as_bytes());
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join(format!("agent-{key}.lock")))?;
        file.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => anyhow::Error::new(ConversationBusy),
            std::fs::TryLockError::Error(error) => anyhow::Error::new(error),
        })?;
        // An orphan may retain its own process after the daemon loses its lock.
        // Refuse to overlap it; PID uncertainty is conservative, never a kill.
        for prior in list(dir)? {
            if prior.session.actor_id.as_deref() == Some(agent)
                && prior.session.is_open()
                && prior.child_pid.is_some_and(process_alive)
            {
                return Err(ConversationBusy.into());
            }
        }
        let path = root.join(format!("{}.json", session.session_id));
        let record = RunRecord {
            writes_consumed: false,
            session,
            owner_pid: std::process::id(),
            child_pid: None,
            interrupted: false,
        };
        atomic_json(&path, &record)?;
        Ok(Self {
            _file: file,
            path,
            record,
        })
    }
    pub fn child(&mut self, pid: u32) -> Result<()> {
        self.record.child_pid = Some(pid);
        atomic_json(&self.path, &self.record)
    }
    fn refresh_child(&mut self) {
        if self.record.child_pid.is_none() {
            if let Ok(bytes) = std::fs::read(&self.path) {
                if let Ok(record) = serde_json::from_slice::<RunRecord>(&bytes) {
                    self.record.child_pid = record.child_pid;
                }
            }
        }
    }
    pub fn finish(&mut self) -> Result<()> {
        self.refresh_child();
        self.record.session.finish();
        atomic_json(&self.path, &self.record)
    }
}
impl Drop for RunLease {
    fn drop(&mut self) {
        self.refresh_child();
        if self.record.session.is_open() {
            self.record.interrupted = true;
            // Preserve an open record when a child may survive cancellation.
            if !self.record.child_pid.is_some_and(process_alive) {
                self.record.session.finish();
            }
            if let Err(e) = atomic_json(&self.path, &self.record) {
                tracing::error!("persisting interrupted run: {e}");
            }
        }
    }
}

pub fn list(dir: &Path) -> Result<Vec<RunRecord>> {
    let root = dir.join("managed_runs");
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut records = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let path = entry?.path();
        if path.extension().is_some_and(|s| s == "json") {
            records.push(
                serde_json::from_slice(&std::fs::read(&path)?)
                    .with_context(|| format!("invalid managed run {}", path.display()))?,
            );
        }
    }
    Ok(records)
}

/// Cross-process device capacity, also used by manual `enox agent run`.
/// A released OS lock with a surviving child remains occupied on recovery.
pub struct DeviceLease {
    _file: File,
}
impl DeviceLease {
    pub async fn acquire(root: &Path, run_path: &Path, limit: usize) -> Result<Self> {
        std::fs::create_dir_all(root)?;
        loop {
            for index in 0..limit.clamp(1, 32) {
                let file = OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .read(true)
                    .write(true)
                    .open(root.join(format!("{index}.lock")))?;
                if file.try_lock().is_err() {
                    continue;
                }
                let pointer = root.join(format!("{index}.json"));
                if let Ok(bytes) = std::fs::read(&pointer) {
                    let previous: PathBuf = serde_json::from_slice(&bytes)?;
                    if let Ok(bytes) = std::fs::read(previous) {
                        let previous: RunRecord = serde_json::from_slice(&bytes)?;
                        if previous.session.is_open()
                            && previous.child_pid.is_some_and(process_alive)
                        {
                            continue;
                        }
                    }
                }
                atomic_json(&pointer, &run_path)?;
                return Ok(Self { _file: file });
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
}

pub fn device_slots_dir() -> Result<PathBuf> {
    #[cfg(test)]
    {
        static ROOT: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
        Ok(ROOT
            .get_or_init(|| tempfile::tempdir().unwrap())
            .path()
            .to_path_buf())
    }
    #[cfg(not(test))]
    {
        Ok(crate::config::enoxian_dir()?.join("execution_slots"))
    }
}

/// Preserve legacy metadata without treating an old timestamp as write evidence.
pub fn migrate_legacy(dir: &Path) -> Result<()> {
    let path = LocalChangeSession::managed_path(dir);
    if !path.exists() {
        return Ok(());
    }
    let session: LocalChangeSession = serde_json::from_slice(&std::fs::read(&path)?)?;
    super::validate_storage_id("legacy run", &session.session_id)?;
    let target = dir
        .join("managed_runs")
        .join(format!("{}.json", session.session_id));
    if !target.exists() {
        let interrupted = session.is_open();
        atomic_json(
            &target,
            &RunRecord {
                writes_consumed: false,
                session,
                owner_pid: 0,
                child_pid: None,
                interrupted,
            },
        )?;
    }
    std::fs::remove_file(path)?;
    Ok(())
}

/// Release only the exact finished run's locks; another run of the same agent
/// or a remote peer's hold must remain intact.
pub fn release_finished_locks(state: &crate::state::AppState) -> Result<()> {
    use yrs::{ReadTxn, Transact};
    for mut record in list(&state.circle_dir)? {
        if record.session.is_open()
            && record.owner_pid != 0
            && !process_alive(record.owner_pid)
            && !record.child_pid.is_some_and(process_alive)
        {
            record.interrupted = true;
            record.session.finish();
            atomic_json(
                &state
                    .circle_dir
                    .join("managed_runs")
                    .join(format!("{}.json", record.session.session_id)),
                &record,
            )?;
        }
    }
    let finished: std::collections::HashSet<_> = list(&state.circle_dir)?
        .into_iter()
        .filter(|r| !r.session.is_open())
        .map(|r| r.session.session_id)
        .collect();
    let mut txn = state
        .control
        .try_transact_mut()
        .map_err(|_| anyhow::anyhow!("Circle busy during lock cleanup"))?;
    let Some(log) = txn.get_array(crate::control::LOCK_LOG_KEY) else {
        return Ok(());
    };
    let holders = crate::control::arbitration::compute_lock_holders(&log, &txn);
    for (path, holder) in holders {
        if holder.peer_id != state.peer_id
            || !holder
                .run_id
                .as_ref()
                .is_some_and(|id| finished.contains(id))
        {
            continue;
        }
        crate::control::arbitration::append_lock_entry(
            &log,
            &mut txn,
            &crate::control::LockEntry {
                entry_id: uuid::Uuid::new_v4().to_string(),
                run_id: holder.run_id,
                agent_id: holder.agent_id.clone(),
                peer_id: holder.peer_id,
                path: path.clone(),
                action: crate::control::LockAction::Release,
                ts: chrono::Utc::now(),
            },
        )?;
        if let Ok(abs) = super::canonical_workspace_path(&state.workspace, Path::new(&path)) {
            if let Ok(metadata) = std::fs::metadata(&abs) {
                let mut permissions = metadata.permissions();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    permissions.set_mode(permissions.mode() | 0o200);
                }
                #[cfg(not(unix))]
                permissions.set_readonly(false);
                std::fs::set_permissions(abs, permissions)?;
            }
        }
        let _ = state
            .events
            .send(crate::control::CircleEvent::LockReleased {
                path,
                agent_id: holder.agent_id,
            });
    }
    Ok(())
}

pub fn ambient_review_required(dir: &Path) -> Result<bool> {
    Ok(list(dir)?.iter().any(|r| {
        !r.writes_consumed && r.session.mode == super::session::SessionMode::AmbientTriggered
    }))
}

pub fn consume_finished(dir: &Path, through: chrono::DateTime<chrono::Utc>) -> Result<()> {
    for mut record in list(dir)? {
        if !record.writes_consumed && record.session.finished_at.is_some_and(|at| at <= through) {
            record.writes_consumed = true;
            atomic_json(
                &dir.join("managed_runs")
                    .join(format!("{}.json", record.session.session_id)),
                &record,
            )?;
        }
    }
    Ok(())
}

fn process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        [pid.to_string(), format!("-{pid}")].iter().any(|target| {
            std::process::Command::new("kill")
                .args(["-0", target])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(true)
        })
    }
    #[cfg(windows)]
    {
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .map(|out| {
                !out.status.success()
                    || String::from_utf8_lossy(&out.stdout).lines().any(|line| {
                        line.split(',')
                            .nth(1)
                            .is_some_and(|id| id.trim_matches('"') == pid.to_string())
                    })
            })
            .unwrap_or(true)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proposal::session::SessionMode;
    fn session(agent: &str) -> LocalChangeSession {
        let mut s =
            LocalChangeSession::start("c".into(), "base".into(), SessionMode::ManagedProcess);
        s.actor_id = Some(agent.into());
        s
    }
    #[test]
    fn independent_agents_and_exclusive_conversation() {
        let d = tempfile::tempdir().unwrap();
        let mut a = RunLease::acquire(d.path(), session("a")).unwrap();
        let b = RunLease::acquire(d.path(), session("b")).unwrap();
        assert!(RunLease::acquire(d.path(), session("a")).is_err());
        a.finish().unwrap();
        drop(a);
        let _next = RunLease::acquire(d.path(), session("a")).unwrap();
        assert!(b.record.session.is_open());
        assert_eq!(list(d.path()).unwrap().len(), 3);
    }
    #[test]
    fn surviving_process_blocks_new_owner() {
        let d = tempfile::tempdir().unwrap();
        let mut a = RunLease::acquire(d.path(), session("a")).unwrap();
        a.child(std::process::id()).unwrap();
        drop(a);
        assert!(RunLease::acquire(d.path(), session("a")).is_err());
        assert!(list(d.path()).unwrap()[0].interrupted);
    }
    #[tokio::test]
    async fn device_slots_cover_independent_launchers_and_recovery() {
        let d = tempfile::tempdir().unwrap();
        let records = tempfile::tempdir().unwrap();
        let path = records.path().join("run.json");
        let first = DeviceLease::acquire(d.path(), &path, 1).await.unwrap();
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(30),
            DeviceLease::acquire(d.path(), &path, 1)
        )
        .await
        .is_err());
        atomic_json(
            &path,
            &RunRecord {
                writes_consumed: false,
                session: session("a"),
                owner_pid: std::process::id(),
                child_pid: Some(std::process::id()),
                interrupted: true,
            },
        )
        .unwrap();
        drop(first);
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(30),
                DeviceLease::acquire(d.path(), &path, 1)
            )
            .await
            .is_err(),
            "an orphaned live child still occupies capacity"
        );
        std::fs::remove_file(&path).unwrap();
        let _next = DeviceLease::acquire(d.path(), &path, 1).await.unwrap();
    }
    #[test]
    fn ambient_review_waits_for_a_scan_after_process_completion() {
        let d = tempfile::tempdir().unwrap();
        let mut session = session("ambient");
        session.mode = crate::proposal::session::SessionMode::AmbientTriggered;
        let mut lease = RunLease::acquire(d.path(), session).unwrap();
        let before_finish = chrono::Utc::now() - chrono::Duration::seconds(1);
        assert!(ambient_review_required(d.path()).unwrap());
        lease.finish().unwrap();
        consume_finished(d.path(), before_finish).unwrap();
        assert!(ambient_review_required(d.path()).unwrap());
        consume_finished(d.path(), chrono::Utc::now()).unwrap();
        assert!(!ambient_review_required(d.path()).unwrap());
    }
}
