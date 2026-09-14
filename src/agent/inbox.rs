//! Recipient-local durable execution inbox. Admission is not completion.
//!
//! Records are committed before acknowledgement or launch. Pending work survives
//! restart; running work becomes interrupted and is never automatically replayed.
//! The persisted activation timestamp prevents migration from replaying old chat.

use crate::control::{ChatMessage, Relay};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};

pub const MAX_PENDING_PER_AGENT: usize = 4;
pub const MAX_PENDING_PER_CIRCLE: usize = 64;
pub const MAX_PENDING_PER_DEVICE: usize = 256;
static PENDING: std::sync::LazyLock<Mutex<BTreeMap<PathBuf, usize>>> =
    std::sync::LazyLock::new(|| Mutex::new(BTreeMap::new()));
fn pending_count(s: &Snapshot) -> usize {
    s.entries
        .iter()
        .filter(|e| e.status == Status::Pending)
        .count()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub agent: String,
    pub mention_key: String,
    pub message: ChatMessage,
    pub task: String,
    pub relay: Option<Relay>,
    pub implicit: bool,
    pub ambient: bool,
}

impl Request {
    fn key(&self) -> String {
        // JSON tuple encoding avoids separator collisions. This inbox is local
        // to one device/Circle, so aliases naming one agent share a recipient.
        serde_json::to_string(&(&self.message.id, &self.agent)).unwrap()
    }

    pub fn delegated(&self) -> bool {
        !self.implicit
            && !self.ambient
            && self.relay.as_ref().and_then(super::relay::poster).is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
    Expired,
    Interrupted,
    LegacySuppressed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub run_id: String,
    pub request: Request,
    pub status: Status,
    pub detail: Option<String>,
    pub admitted_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    version: u32,
    pub activated_at: i64,
    // In admission order; terminal entries retain dedup evidence.
    pub entries: Vec<Entry>,
    charges: BTreeMap<String, u8>,
}

pub struct Inbox {
    _owner: std::fs::File,
    path: PathBuf,
    snapshot: Mutex<Snapshot>,
    healthy: AtomicBool,
}

pub enum Admission {
    Duplicate,
    Rejected(Entry),
    Accepted { entry: Entry, displaced: Vec<Entry> },
}

/// Take the exclusive owner lock, waiting out a departing owner's fork window.
///
/// `flock` belongs to the open file description, not the fd, so a child forked
/// between the owner's open and that child's `exec` keeps the lock alive after
/// the owner closes its own fd. `O_CLOEXEC` clears the fd at `exec`, but the
/// window before it is real, and this daemon forks constantly to launch agents,
/// so a restart can lose the race against its own predecessor's release. A
/// genuine second owner instead holds the lock for its whole lifetime, so only
/// this sub-millisecond window is worth waiting out before reporting a conflict.
fn take_ownership(owner: &std::fs::File) -> Result<()> {
    const ATTEMPTS: u32 = 100;
    const PAUSE: std::time::Duration = std::time::Duration::from_millis(10);
    for remaining in (0..ATTEMPTS).rev() {
        match owner.try_lock() {
            Ok(()) => return Ok(()),
            Err(std::fs::TryLockError::WouldBlock) if remaining > 0 => std::thread::sleep(PAUSE),
            Err(error) => {
                return Err(anyhow::Error::new(error)
                    .context("execution inbox already has an active owner"))
            }
        }
    }
    unreachable!("the final attempt either takes the lock or reports the conflict")
}

impl Inbox {
    pub fn path(circle_dir: &Path) -> PathBuf {
        circle_dir.join("execution_inbox.json")
    }

    pub fn read(circle_dir: &Path) -> Result<Option<Snapshot>> {
        match std::fs::read(Self::path(circle_dir)) {
            Ok(bytes) => {
                let mut snapshot: Snapshot =
                    serde_json::from_slice(&bytes).context("invalid execution inbox")?;
                if snapshot.version != 1 {
                    bail!("unsupported execution inbox version {}", snapshot.version);
                }
                // Earlier imports mistook an internal dedup marker for an
                // agent name. Repair metadata without replaying old observations.
                for entry in &mut snapshot.entries {
                    if let Some(agent) = entry.request.agent.strip_prefix("~ambient:") {
                        entry.request.agent = agent.to_string();
                        entry.request.ambient = true;
                        if entry.status == Status::Pending {
                            entry.status = Status::Cancelled;
                            entry.detail = Some(
                                "Older listening request; send a new message to ask again".into(),
                            );
                        }
                    }
                }
                Ok(Some(snapshot))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Called once by the owning reaction loop, never by read-only API callers.
    pub fn open(circle_dir: &Path, now: i64) -> Result<Self> {
        std::fs::create_dir_all(circle_dir)?;
        let owner = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(circle_dir.join("execution_inbox.lock"))?;
        take_ownership(&owner)?;
        let mut snapshot = Self::read(circle_dir)?.unwrap_or(Snapshot {
            version: 1,
            activated_at: now - 2,
            entries: Vec::new(),
            charges: BTreeMap::new(),
        });
        for entry in &mut snapshot.entries {
            let change = match entry.status {
                Status::Running => Some((
                    Status::Interrupted,
                    "daemon restarted during execution; automatic retry suppressed",
                )),
                Status::Pending if entry.request.ambient => {
                    Some((Status::Expired, "ambient observation expired on restart"))
                }
                _ => None,
            };
            if let Some((status, detail)) = change {
                entry.status = status;
                entry.detail = Some(detail.into());
                entry.updated_at = now;
            }
        }
        let inbox = Self {
            _owner: owner,
            path: Self::path(circle_dir),
            snapshot: Mutex::new(snapshot.clone()),
            healthy: AtomicBool::new(true),
        };
        let mut aggregate = PENDING.lock().unwrap();
        let available = MAX_PENDING_PER_DEVICE.saturating_sub(aggregate.values().sum());
        for entry in snapshot
            .entries
            .iter_mut()
            .filter(|e| e.status == Status::Pending)
            .skip(available)
        {
            entry.status = Status::Expired;
            entry.detail = Some("device queue capacity exceeded during recovery".into());
            entry.updated_at = now;
        }
        if let Err(error) = inbox.persist(&snapshot) {
            drop(aggregate);
            return Err(error);
        }
        *inbox.snapshot.lock().unwrap() = snapshot.clone();
        aggregate.insert(inbox.path.clone(), pending_count(&snapshot));
        drop(aggregate);
        Ok(inbox)
    }

    pub fn activated_at(&self) -> i64 {
        self.snapshot.lock().unwrap().activated_at
    }
    pub fn entries(&self) -> Vec<Entry> {
        self.snapshot.lock().unwrap().entries.clone()
    }
    pub fn contains(&self, request: &Request) -> bool {
        self.snapshot
            .lock()
            .unwrap()
            .entries
            .iter()
            .any(|e| e.request.key() == request.key())
    }

    /// Explicit retries create a new attempt; the old outcome remains visible.
    pub fn retry(&self, run_id: &str, max: u8, now: i64) -> Result<Entry> {
        let mut aggregate = PENDING.lock().unwrap();
        anyhow::ensure!(
            aggregate.values().sum::<usize>() < MAX_PENDING_PER_DEVICE,
            "device queue full"
        );
        let mut current = self.snapshot.lock().unwrap();
        let old = current
            .entries
            .iter()
            .find(|e| e.run_id == run_id)
            .context("unknown run")?;
        anyhow::ensure!(
            !(old.status == Status::LegacySuppressed && old.request.ambient),
            "older listening requests cannot be replayed; send a new message instead"
        );
        anyhow::ensure!(
            matches!(
                old.status,
                Status::Failed
                    | Status::Interrupted
                    | Status::Expired
                    | Status::Cancelled
                    | Status::LegacySuppressed
            ),
            "only unsuccessful terminal runs can be retried"
        );
        anyhow::ensure!(
            !current
                .entries
                .iter()
                .any(|e| e.request.key() == old.request.key()
                    && matches!(e.status, Status::Pending | Status::Running)),
            "a retry is already pending or running"
        );
        anyhow::ensure!(
            current
                .entries
                .iter()
                .filter(|e| e.status == Status::Pending && e.request.agent == old.request.agent)
                .count()
                < MAX_PENDING_PER_AGENT,
            "agent queue full"
        );
        anyhow::ensure!(
            current
                .entries
                .iter()
                .filter(|e| e.status == Status::Pending)
                .count()
                < MAX_PENDING_PER_CIRCLE,
            "Circle queue full"
        );
        let mut entry = old.clone();
        entry.run_id = uuid::Uuid::new_v4().to_string();
        entry.status = Status::Pending;
        entry.detail = Some(format!("explicit retry of {run_id}"));
        entry.admitted_at = now;
        entry.updated_at = now;
        let mut next = current.clone();
        if entry.request.delegated() {
            let relay = entry.request.relay.as_ref().unwrap();
            let spent = next.charges.entry(relay.root.clone()).or_default();
            anyhow::ensure!(
                super::relay::has_budget(relay, max)
                    && *spent < max.min(super::relay::RELAY_TURNS_CEILING),
                "relay budget spent"
            );
            *spent += 1;
        }
        next.entries.push(entry.clone());
        self.persist(&next)?;
        aggregate.insert(self.path.clone(), pending_count(&next));
        *current = next;
        Ok(entry)
    }

    pub fn record_suppressed(&self, request: Request, now: i64) -> Result<()> {
        let mut current = self.snapshot.lock().unwrap();
        if current
            .entries
            .iter()
            .any(|e| e.request.key() == request.key())
        {
            return Ok(());
        }
        let mut next = current.clone();
        next.entries.push(Entry {
            run_id: uuid::Uuid::new_v4().to_string(),
            request,
            status: Status::LegacySuppressed,
            detail: Some("legacy handled marker; execution outcome unknown".into()),
            admitted_at: now,
            updated_at: now,
        });
        self.persist(&next)?;
        *current = next;
        Ok(())
    }

    pub fn admit(&self, request: Request, max_relay_turns: u8, now: i64) -> Result<Admission> {
        self.admit_with_limit(request, max_relay_turns, now, MAX_PENDING_PER_DEVICE)
    }

    fn admit_with_limit(
        &self,
        request: Request,
        max_relay_turns: u8,
        now: i64,
        device_limit: usize,
    ) -> Result<Admission> {
        let mut aggregate = PENDING.lock().unwrap();
        let mut current = self.snapshot.lock().unwrap();
        if current
            .entries
            .iter()
            .any(|e| e.request.key() == request.key())
        {
            return Ok(Admission::Duplicate);
        }
        let mut next = current.clone();
        let mut entry = Entry {
            run_id: uuid::Uuid::new_v4().to_string(),
            request,
            status: Status::Pending,
            detail: None,
            admitted_at: now,
            updated_at: now,
        };
        let mut displaced = Vec::new();
        if aggregate.values().sum::<usize>() >= device_limit {
            entry.status = Status::Expired;
            entry.detail = Some("device queue capacity exceeded".into());
            next.entries.push(entry.clone());
            self.persist(&next)?;
            aggregate.insert(self.path.clone(), pending_count(&next));
            *current = next;
            return Ok(Admission::Rejected(entry));
        }
        let agent_pending = next
            .entries
            .iter()
            .filter(|e| e.status == Status::Pending && e.request.agent == entry.request.agent)
            .count();
        if agent_pending >= MAX_PENDING_PER_AGENT {
            if let Some(old) = next
                .entries
                .iter_mut()
                .find(|e| e.status == Status::Pending && e.request.agent == entry.request.agent)
            {
                old.status = Status::Expired;
                old.detail = Some("per-agent queue capacity exceeded".into());
                old.updated_at = now;
                displaced.push(old.clone());
            }
        }
        if next
            .entries
            .iter()
            .filter(|e| e.status == Status::Pending)
            .count()
            >= MAX_PENDING_PER_CIRCLE
        {
            if let Some(old) = next
                .entries
                .iter_mut()
                .find(|e| e.status == Status::Pending)
            {
                old.status = Status::Expired;
                old.detail = Some("Circle queue capacity exceeded".into());
                old.updated_at = now;
                displaced.push(old.clone());
            }
        }
        // Displaced pending turns never launched, so return their reservations.
        for old in &displaced {
            if old.request.delegated() {
                let root = &old.request.relay.as_ref().unwrap().root;
                if let Some(spent) = next.charges.get_mut(root) {
                    *spent = spent.saturating_sub(1);
                }
            }
        }
        if entry.request.delegated() {
            let relay = entry.request.relay.as_ref().unwrap();
            let max = max_relay_turns.min(super::relay::RELAY_TURNS_CEILING);
            let spent = next.charges.entry(relay.root.clone()).or_default();
            if !super::relay::has_budget(relay, max) || *spent >= max {
                entry.status = Status::Cancelled;
                entry.detail = Some("relay budget spent".into());
                next = current.clone();
                next.entries.push(entry.clone());
                self.persist(&next)?;
                aggregate.insert(self.path.clone(), pending_count(&next));
                *current = next;
                return Ok(Admission::Rejected(entry));
            }
            *spent += 1;
        }
        next.entries.push(entry.clone());
        self.persist(&next)?;
        aggregate.insert(self.path.clone(), pending_count(&next));
        *current = next;
        Ok(Admission::Accepted { entry, displaced })
    }

    pub fn annotate_running(&self, run_id: &str, detail: Option<String>) -> Result<()> {
        let mut current = self.snapshot.lock().unwrap();
        let mut next = current.clone();
        let Some(entry) = next
            .entries
            .iter_mut()
            .find(|e| e.run_id == run_id && e.status == Status::Running)
        else {
            return Ok(());
        };
        if entry.detail == detail {
            return Ok(());
        }
        entry.detail = detail;
        entry.updated_at = chrono::Utc::now().timestamp();
        self.persist(&next)?;
        *current = next;
        Ok(())
    }

    pub fn transition(
        &self,
        run_id: &str,
        expected: Status,
        status: Status,
        detail: Option<String>,
        now: i64,
    ) -> Result<bool> {
        anyhow::ensure!(
            matches!(
                (expected, status),
                (
                    Status::Pending,
                    Status::Running | Status::Cancelled | Status::Expired
                ) | (
                    Status::Running,
                    Status::Completed | Status::Failed | Status::Interrupted | Status::Pending
                )
            ),
            "invalid execution state transition"
        );
        let mut aggregate = PENDING.lock().unwrap();
        let mut current = self.snapshot.lock().unwrap();
        let mut next = current.clone();
        let Some(entry) = next
            .entries
            .iter_mut()
            .find(|e| e.run_id == run_id && e.status == expected)
        else {
            return Ok(false);
        };
        entry.status = status;
        entry.detail = detail;
        entry.updated_at = now;
        if status == Status::Pending {
            let agent = next
                .entries
                .iter()
                .find(|e| e.run_id == run_id)
                .unwrap()
                .request
                .agent
                .clone();
            if pending_count(&next) > MAX_PENDING_PER_CIRCLE
                || aggregate.values().sum::<usize>() >= MAX_PENDING_PER_DEVICE
                || next
                    .entries
                    .iter()
                    .filter(|e| e.status == Status::Pending && e.request.agent == agent)
                    .count()
                    > MAX_PENDING_PER_AGENT
            {
                let entry = next
                    .entries
                    .iter_mut()
                    .find(|e| e.run_id == run_id)
                    .unwrap();
                entry.status = Status::Expired;
                entry.detail =
                    Some("queue capacity exceeded while waiting for the conversation".into());
            }
        }
        self.persist(&next)?;
        aggregate.insert(self.path.clone(), pending_count(&next));
        *current = next;
        Ok(true)
    }

    pub fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::SeqCst)
    }

    fn persist(&self, snapshot: &Snapshot) -> Result<()> {
        use std::io::Write;
        let parent = self.path.parent().unwrap();
        std::fs::create_dir_all(parent)?;
        // Unique temporary file, durable data, atomic replacement. A failed
        // commit cannot acknowledge a request or mutate the in-memory snapshot.
        if !self.healthy.swap(false, Ordering::SeqCst) {
            bail!("execution inbox persistence failed; owner recovery required before further execution");
        }
        let tmp_path = parent.join(format!(".inbox-{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&tmp_path)?;
            serde_json::to_writer(&mut file, snapshot)?;
            file.flush()?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&tmp_path, &self.path)?;
            #[cfg(unix)]
            std::fs::File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if result.is_ok() {
            self.healthy.store(true, Ordering::SeqCst);
        } else {
            let _ = std::fs::remove_file(tmp_path);
        }
        result
    }
}

impl Drop for Inbox {
    fn drop(&mut self) {
        PENDING.lock().unwrap().remove(&self.path);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::control::Author;

    pub(crate) fn request(id: &str, agent: &str) -> Request {
        Request {
            agent: agent.into(),
            mention_key: agent.into(),
            task: "do this".into(),
            message: ChatMessage {
                thread_root: None,
                id: id.into(),
                agent_id: "human".into(),
                text: "do this".into(),
                mentions: vec![agent.into()],
                ts: 100,
                peer_id: "sender".into(),
                attachments: vec![],
                relay: Some(crate::agent::relay::mint(id, "sender")),
                author: Author::Human,
                reply_to: None,
            },
            relay: Some(crate::agent::relay::mint(id, "sender")),
            implicit: false,
            ambient: false,
        }
    }

    fn accepted(inbox: &Inbox, req: Request) -> Entry {
        match inbox.admit(req, 20, 100).unwrap() {
            Admission::Accepted { entry, .. } => entry,
            _ => panic!("expected admission"),
        }
    }

    #[test]
    fn pending_survives_restart_and_completion_suppresses_replay() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let entry = accepted(&inbox, request("m1", "claude"));
        drop(inbox);
        let inbox = Inbox::open(dir.path(), 900).unwrap();
        assert_eq!(
            inbox.activated_at(),
            98,
            "activation must not move on restart"
        );
        assert_eq!(inbox.entries()[0].status, Status::Pending);
        assert!(inbox
            .transition(&entry.run_id, Status::Pending, Status::Running, None, 901)
            .unwrap());
        assert!(inbox
            .transition(&entry.run_id, Status::Running, Status::Completed, None, 902)
            .unwrap());
        drop(inbox);
        let inbox = Inbox::open(dir.path(), 1000).unwrap();
        assert_eq!(inbox.entries()[0].status, Status::Completed);
        assert!(matches!(
            inbox.admit(request("m1", "claude"), 20, 1001).unwrap(),
            Admission::Duplicate
        ));
    }

    #[test]
    fn crash_does_not_retry_side_effects_and_old_ambient_is_not_work() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let active = accepted(&inbox, request("running", "claude"));
        inbox
            .transition(&active.run_id, Status::Pending, Status::Running, None, 101)
            .unwrap();
        accepted(&inbox, request("pending", "codex"));
        let mut ambient = request("ambient", "suzent");
        ambient.ambient = true;
        accepted(&inbox, ambient);
        drop(inbox);
        let inbox = Inbox::open(dir.path(), 200).unwrap();
        assert_eq!(
            inbox.entries().iter().map(|e| e.status).collect::<Vec<_>>(),
            [Status::Interrupted, Status::Pending, Status::Expired]
        );
        assert!(matches!(
            inbox.admit(request("running", "claude"), 20, 201).unwrap(),
            Admission::Duplicate
        ));
    }

    #[test]
    fn aliases_share_one_job_but_different_agents_do_not() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        accepted(&inbox, request("message", "claude"));
        let mut alias = request("message", "claude");
        alias.mention_key = "owner/device/claude".into();
        assert!(matches!(
            inbox.admit(alias, 20, 100).unwrap(),
            Admission::Duplicate
        ));
        accepted(&inbox, request("message", "codex"));
        assert_eq!(inbox.entries().len(), 2);
    }

    #[test]
    fn overflow_is_durable_and_preserves_other_agents_order() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        for i in 0..4 {
            accepted(&inbox, request(&format!("c{i}"), "claude"));
        }
        accepted(&inbox, request("x", "codex"));
        match inbox.admit(request("c4", "claude"), 20, 105).unwrap() {
            Admission::Accepted { displaced, .. } => {
                assert_eq!(displaced.len(), 1);
                assert_eq!(displaced[0].request.message.id, "c0");
                assert_eq!(displaced[0].status, Status::Expired);
            }
            _ => panic!("expected admission"),
        }
        drop(inbox);
        let inbox = Inbox::open(dir.path(), 200).unwrap();
        assert_eq!(
            inbox
                .entries()
                .iter()
                .filter(|e| e.status == Status::Pending)
                .map(|e| e.request.message.id.as_str())
                .collect::<Vec<_>>(),
            ["c1", "c2", "c3", "x", "c4"]
        );
        assert!(matches!(
            inbox.admit(request("c0", "claude"), 20, 201).unwrap(),
            Admission::Duplicate
        ));
    }

    #[test]
    fn ledger_survives_restart_and_duplicate_delivery_does_not_spend() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let mut req = request("m1", "codex");
        req.relay = Some(crate::agent::relay::extend(
            &crate::agent::relay::mint("root", "sender"),
            "claude",
        ));
        assert!(matches!(
            inbox.admit(req.clone(), 2, 100).unwrap(),
            Admission::Accepted { .. }
        ));
        for _ in 0..10 {
            assert!(matches!(
                inbox.admit(req.clone(), 2, 100).unwrap(),
                Admission::Duplicate
            ));
        }
        drop(inbox);
        let inbox = Inbox::open(dir.path(), 200).unwrap();
        req.message.id = "m2".into();
        assert!(matches!(
            inbox.admit(req.clone(), 2, 200).unwrap(),
            Admission::Accepted { .. }
        ));
        req.message.id = "m3".into();
        assert!(matches!(
            inbox.admit(req, 2, 201).unwrap(),
            Admission::Rejected(_)
        ));
    }

    #[test]
    fn aggregate_backlog_and_relay_hard_ceiling_remain_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        for i in 0..MAX_PENDING_PER_CIRCLE + 1 {
            accepted(&inbox, request(&format!("m{i}"), &format!("agent{i}")));
        }
        assert_eq!(
            inbox
                .entries()
                .iter()
                .filter(|e| e.status == Status::Pending)
                .count(),
            MAX_PENDING_PER_CIRCLE
        );
        assert_eq!(inbox.entries()[0].status, Status::Expired);
        let mut delegated = request("d0", "codex");
        delegated.relay = Some(crate::agent::relay::extend(
            &crate::agent::relay::mint("chain", "sender"),
            "claude",
        ));
        for i in 0..super::super::relay::RELAY_TURNS_CEILING {
            delegated.message.id = format!("d{i}");
            let Admission::Accepted { entry, .. } =
                inbox.admit(delegated.clone(), 250, 101).unwrap()
            else {
                panic!("expected admission");
            };
            inbox
                .transition(&entry.run_id, Status::Pending, Status::Running, None, 101)
                .unwrap();
            inbox
                .transition(&entry.run_id, Status::Running, Status::Completed, None, 101)
                .unwrap();
        }
        delegated.message.id = "over-budget".into();
        assert!(matches!(
            inbox.admit(delegated, 250, 102).unwrap(),
            Admission::Rejected(_)
        ));
    }

    #[test]
    fn only_one_owner_and_corruption_never_resets_history() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        assert!(Inbox::open(dir.path(), 100).is_err());
        drop(inbox);
        std::fs::write(Inbox::path(dir.path()), b"invalid json").unwrap();
        assert!(Inbox::open(dir.path(), 200).is_err());
        assert_eq!(
            std::fs::read(Inbox::path(dir.path())).unwrap(),
            b"invalid json"
        );
    }

    #[test]
    fn failed_persistence_cannot_acknowledge_or_launch_work() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        // Force atomic replacement to fail without relying on Unix permissions.
        std::fs::remove_file(Inbox::path(dir.path())).unwrap();
        std::fs::create_dir(Inbox::path(dir.path())).unwrap();
        assert!(inbox.admit(request("m", "claude"), 20, 100).is_err());
        assert!(inbox.entries().is_empty());
        std::fs::remove_dir(Inbox::path(dir.path())).unwrap();
        assert!(
            inbox.admit(request("n", "claude"), 20, 100).is_err(),
            "uncertain commits require recovery"
        );
    }
    #[test]
    fn retry_retains_failed_attempt_and_refuses_duplicate_retry() {
        let d = tempfile::tempdir().unwrap();
        let inbox = Inbox::open(d.path(), 100).unwrap();
        inbox.admit(request("m", "a"), 20, 100).unwrap();
        let id = inbox.entries()[0].run_id.clone();
        inbox
            .transition(&id, Status::Pending, Status::Running, None, 101)
            .unwrap();
        inbox
            .transition(
                &id,
                Status::Running,
                Status::Failed,
                Some("exit 7".into()),
                102,
            )
            .unwrap();
        let retry = inbox.retry(&id, 20, 103).unwrap();
        assert_ne!(retry.run_id, id);
        assert_eq!(inbox.entries()[0].status, Status::Failed);
        assert!(inbox.retry(&id, 20, 104).is_err());
    }
    fn delegated(id: &str) -> Request {
        let mut req = request(id, "codex");
        req.relay = Some(crate::agent::relay::extend(
            &crate::agent::relay::mint("root", "sender"),
            "claude",
        ));
        req
    }

    #[test]
    fn rejected_capacity_does_not_spend_and_displacement_refunds() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let Admission::Rejected(rejected) = inbox
            .admit_with_limit(delegated("full"), 2, 100, 0)
            .unwrap()
        else {
            panic!("expected capacity rejection");
        };
        assert_eq!(inbox.snapshot.lock().unwrap().charges.get("root"), None);
        let retry = inbox.retry(&rejected.run_id, 2, 101).unwrap();
        assert_eq!(retry.status, Status::Pending);
        for i in 0..10 {
            assert!(matches!(
                inbox.admit(delegated(&format!("m{i}")), 5, 102).unwrap(),
                Admission::Accepted { .. }
            ));
        }
        assert_eq!(inbox.snapshot.lock().unwrap().charges["root"], 4);
    }

    #[test]
    fn retry_cannot_exceed_the_hard_relay_ceiling() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let mut entry = accepted(&inbox, delegated("retry"));
        for attempt in 1..=crate::agent::relay::RELAY_TURNS_CEILING {
            inbox
                .transition(&entry.run_id, Status::Pending, Status::Running, None, 100)
                .unwrap();
            inbox
                .transition(&entry.run_id, Status::Running, Status::Failed, None, 100)
                .unwrap();
            if attempt < crate::agent::relay::RELAY_TURNS_CEILING {
                entry = inbox.retry(&entry.run_id, 250, 100).unwrap();
            }
        }
        assert!(inbox.retry(&entry.run_id, 250, 100).is_err());
        assert_eq!(
            inbox.snapshot.lock().unwrap().charges["root"],
            crate::agent::relay::RELAY_TURNS_CEILING
        );
    }
    #[test]
    fn previously_imported_ambient_names_are_repaired_without_replaying() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        inbox
            .record_suppressed(request("old", "~ambient:claude"), 100)
            .unwrap();
        inbox
            .admit(request("bad-retry", "~ambient:codex"), 20, 100)
            .unwrap();
        drop(inbox);
        let inbox = Inbox::open(dir.path(), 200).unwrap();
        let entries = inbox.entries();
        assert_eq!(entries[0].request.agent, "claude");
        assert_eq!(entries[0].status, Status::LegacySuppressed);
        assert_eq!(entries[1].request.agent, "codex");
        assert_eq!(entries[1].status, Status::Cancelled);
        assert!(entries.iter().all(|e| e.request.ambient));
    }
}
