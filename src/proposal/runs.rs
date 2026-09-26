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
                    .with_context(|| format!("invalid managed run {}; restore this record from backup after verifying its agent process has stopped (ignoring it could overlap a surviving turn)", path.display()))?,
            );
        }
    }
    Ok(records)
}

/// Cross-process device capacity, also used by manual `enox agent run`.
/// A released OS lock with a surviving child remains occupied on recovery.
#[derive(Debug, thiserror::Error)]
#[error("waiting for device capacity (cancelled or timed out before launch)")]
pub struct DeviceCapacityUnavailable;

pub struct DeviceLease {
    _file: File,
}
impl DeviceLease {
    pub async fn acquire(root: &Path, run_path: &Path, limit: usize) -> Result<Self> {
        Self::acquire_cancellable(
            root,
            run_path,
            limit,
            &tokio_util::sync::CancellationToken::new(),
            std::time::Duration::from_secs(30),
        )
        .await
    }

    pub async fn acquire_cancellable(
        root: &Path,
        run_path: &Path,
        limit: usize,
        cancel: &tokio_util::sync::CancellationToken,
        wait: std::time::Duration,
    ) -> Result<Self> {
        std::fs::create_dir_all(root)?;
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            if cancel.is_cancelled() || tokio::time::Instant::now() >= deadline {
                return Err(DeviceCapacityUnavailable.into());
            }
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
            tokio::select! {
                _ = cancel.cancelled() => return Err(DeviceCapacityUnavailable.into()),
                _ = tokio::time::sleep_until(deadline) => return Err(DeviceCapacityUnavailable.into()),
                _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {},
            }
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

/// Renew a running run's lock once less than this is left on its lease.
///
/// Half the default lease: the reconcile tick runs every few seconds, so a
/// lock is renewed about every five minutes and never gets close to lapsing.
const RENEW_WITHIN_SECS: i64 = crate::control::arbitration::DEFAULT_LOCK_SECS / 2;

/// The entry that extends `holder`'s lease on `path`, when a run still going
/// holds it and the lease is running low. A run can outlast any lease, and it
/// should not have to know to renew: the daemon knows it is alive.
fn renewal(
    path: &str,
    holder: &crate::control::arbitration::LockHolder,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<crate::control::LockEntry> {
    if holder.expires_at - now >= chrono::Duration::seconds(RENEW_WITHIN_SECS) {
        return None;
    }
    Some(crate::control::LockEntry {
        entry_id: uuid::Uuid::new_v4().to_string(),
        run_id: holder.run_id.clone(),
        agent_id: holder.agent_id.clone(),
        peer_id: holder.peer_id.clone(),
        path: path.to_string(),
        action: crate::control::LockAction::Acquire,
        ts: now,
        expires_at: Some(
            now + chrono::Duration::seconds(crate::control::arbitration::DEFAULT_LOCK_SECS),
        ),
        takeover: false,
        taken_over_from: holder.taken_over_from.clone(),
        taken_over_from_peer_id: holder.taken_over_from_peer_id.clone(),
    })
}

/// Release only the exact finished run's locks, and keep a running run's
/// locks renewed; another run of the same agent or a remote peer's hold must
/// remain intact.
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
    let (open, finished): (Vec<_>, Vec<_>) = list(&state.circle_dir)?
        .into_iter()
        .partition(|r| r.session.is_open());
    let open: std::collections::HashSet<_> =
        open.into_iter().map(|r| r.session.session_id).collect();
    let finished: std::collections::HashSet<_> =
        finished.into_iter().map(|r| r.session.session_id).collect();
    let mut txn = state
        .control
        .try_transact_mut()
        .map_err(|_| anyhow::anyhow!("Circle busy during lock cleanup"))?;
    let Some(log) = txn.get_array(crate::control::LOCK_LOG_KEY) else {
        drop(txn);
        return prune_consumed(&state.circle_dir, chrono::Utc::now());
    };
    let holders = crate::control::arbitration::compute_lock_holders(&log, &txn);
    let now = chrono::Utc::now();
    for (path, holder) in holders {
        if holder.peer_id != state.peer_id {
            continue;
        }
        let Some(run_id) = holder.run_id.as_ref() else {
            continue;
        };
        if open.contains(run_id) {
            if let Some(entry) = renewal(&path, &holder, now) {
                let expires_at = entry.expires_at.unwrap_or(now);
                crate::control::arbitration::append_lock_entry(&log, &mut txn, &entry)?;
                let _ = state
                    .events
                    .send(crate::control::CircleEvent::LockAcquired {
                        path,
                        agent_id: holder.agent_id,
                        expires_at,
                        taken_over_from: None,
                    });
            }
            continue;
        }
        if !finished.contains(run_id) {
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
                expires_at: None,
                takeover: false,
                taken_over_from: None,
                taken_over_from_peer_id: None,
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
    drop(txn);
    prune_consumed(&state.circle_dir, chrono::Utc::now())
}

// Retain a bounded recent history once both process completion and workspace
// capture are durable. Active/unconsumed records are never age-pruned.
fn prune_consumed(dir: &Path, now: chrono::DateTime<chrono::Utc>) -> Result<()> {
    let mut candidates: Vec<_> = list(dir)?
        .into_iter()
        .filter(|r| r.writes_consumed && !r.session.is_open())
        .collect();
    candidates.sort_by_key(|r| std::cmp::Reverse(r.session.finished_at));
    for (index, record) in candidates.into_iter().enumerate() {
        let old = record
            .session
            .finished_at
            .is_some_and(|at| at < now - chrono::Duration::days(30));
        if old || index >= 1000 {
            // A pending pre-launch turn can reuse its run id. Serialize with
            // that agent and re-read before deleting a previously closed record.
            let Some(agent) = record.session.actor_id.as_deref() else {
                continue;
            };
            let root = dir.join("managed_runs");
            let key = super::blob::BlobStore::hash(agent.as_bytes());
            let lock = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(root.join(format!("agent-{key}.lock")))?;
            if lock.try_lock().is_err() {
                continue;
            }
            let path = root.join(format!("{}.json", record.session.session_id));
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let current: RunRecord = serde_json::from_slice(&bytes)?;
            if current.writes_consumed
                && !current.session.is_open()
                && current.session.finished_at == record.session.finished_at
                && !current.child_pid.is_some_and(process_alive)
            {
                std::fs::remove_file(path)?;
            }
        }
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
        // `--` so a negative pid is a process group, not an option; see
        // `agent::spawn::kill_tree` for what procps-ng does without it.
        [pid.to_string(), format!("-{pid}")].iter().any(|target| {
            std::process::Command::new("kill")
                .args(["-0", "--", target])
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
    #[tokio::test]
    async fn device_capacity_wait_is_cancellable_and_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.json");
        let _held = DeviceLease::acquire(dir.path(), &path, 1).await.unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        let wait = DeviceLease::acquire_cancellable(
            dir.path(),
            &path,
            1,
            &cancel,
            std::time::Duration::from_secs(30),
        );
        let stop = async {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            cancel.cancel();
        };
        let (result, ()) = tokio::join!(wait, stop);
        assert!(result.err().unwrap().is::<DeviceCapacityUnavailable>());
        let result = DeviceLease::acquire_cancellable(
            dir.path(),
            &path,
            1,
            &tokio_util::sync::CancellationToken::new(),
            std::time::Duration::from_millis(10),
        )
        .await;
        assert!(result.err().unwrap().is::<DeviceCapacityUnavailable>());
    }

    #[test]
    fn retention_keeps_unconsumed_and_active_records() {
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now();
        for (agent, consumed, finished) in [
            ("retired", true, true),
            ("uncaptured", false, true),
            ("active", false, false),
        ] {
            let mut s = session(agent);
            if finished {
                s.finished_at = Some(now - chrono::Duration::days(31));
            }
            atomic_json(
                &dir.path()
                    .join("managed_runs")
                    .join(format!("{}.json", s.session_id)),
                &RunRecord {
                    session: s,
                    writes_consumed: consumed,
                    owner_pid: 0,
                    child_pid: None,
                    interrupted: false,
                },
            )
            .unwrap();
        }
        prune_consumed(dir.path(), now).unwrap();
        let records = list(dir.path()).unwrap();
        assert_eq!(records.len(), 2);
        assert!(records
            .iter()
            .all(|r| r.session.actor_id.as_deref() != Some("retired")));
    }

    #[test]
    fn corrupt_run_is_preserved_with_actionable_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("managed_runs/broken.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "truncated").unwrap();
        let error = RunLease::acquire(dir.path(), session("a")).err().unwrap();
        assert!(error.to_string().contains("broken.json"));
        assert!(error.to_string().contains("surviving turn"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "truncated");
    }

    fn held(
        expires_in_secs: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> crate::control::arbitration::LockHolder {
        crate::control::arbitration::LockHolder {
            run_id: Some("run-1".into()),
            agent_id: "codex".into(),
            peer_id: "peer-a".into(),
            expires_at: now + chrono::Duration::seconds(expires_in_secs),
            taken_over_from: Some("hermes".into()),
            taken_over_from_peer_id: Some("peer-b".into()),
        }
    }

    #[test]
    fn a_running_lock_with_plenty_of_lease_left_is_not_renewed() {
        let now = chrono::Utc::now();
        assert!(renewal("a.rs", &held(RENEW_WITHIN_SECS + 1, now), now).is_none());
    }

    #[test]
    fn a_running_lock_running_low_is_renewed_as_the_same_lock() {
        let now = chrono::Utc::now();
        let holder = held(RENEW_WITHIN_SECS - 1, now);

        let entry = renewal("a.rs", &holder, now).expect("lease running low");

        assert_eq!(entry.action, crate::control::LockAction::Acquire);
        assert_eq!(entry.path, "a.rs");
        assert_eq!(entry.run_id.as_deref(), Some("run-1"));
        assert_eq!(entry.agent_id, "codex");
        assert_eq!(entry.peer_id, "peer-a");
        assert!(!entry.takeover);
        assert_eq!(entry.taken_over_from.as_deref(), Some("hermes"));
        assert_eq!(
            entry.expires_at,
            Some(now + chrono::Duration::seconds(crate::control::arbitration::DEFAULT_LOCK_SECS))
        );
    }

    #[test]
    fn a_renewal_keeps_the_run_holding_its_lock() {
        use yrs::{Doc, Transact};
        let now = chrono::Utc::now();
        let doc = Doc::new();
        let log = doc.get_or_insert_array(crate::control::LOCK_LOG_KEY);
        let holder = held(60, now);
        {
            let mut txn = doc.transact_mut();
            // The bind that started it, ten minutes' lease from 540 s ago.
            let bound_at = now - chrono::Duration::seconds(540);
            let first = renewal("a.rs", &held(0, bound_at), bound_at).unwrap();
            crate::control::arbitration::append_lock_entry(&log, &mut txn, &first).unwrap();
            let renewed = renewal("a.rs", &holder, now).unwrap();
            crate::control::arbitration::append_lock_entry(&log, &mut txn, &renewed).unwrap();
        }
        let txn = doc.transact();
        // Past the original lease, the run still holds the path.
        let later = now + chrono::Duration::seconds(300);
        let holders = crate::control::arbitration::compute_lock_holders_at(&log, &txn, later);
        assert_eq!(
            holders.get("a.rs").map(|h| h.agent_id.as_str()),
            Some("codex")
        );
    }
}
