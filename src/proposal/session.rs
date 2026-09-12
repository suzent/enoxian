//! Local change sessions: a declared or detected period of local work that
//! proposals can be attributed to.
//!
//! A session never grants authority — it only improves attribution. The
//! filesystem mutation, not the session, is what creates the proposal.

use super::model::Confidence;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    /// Default: changes detected in the normal workspace with no session.
    Ambient,
    /// A chat mention opened this session; the agent edits the normal
    /// workspace unless explicitly sandboxed.
    AmbientTriggered,
    /// enoxian launched the agent as a child process (`enox agent run`).
    ManagedProcess,
    /// The user declared the actor (`enox session start --actor ...`).
    ClaimedSession,
    /// The agent works in a forked workspace owned by enoxian.
    Sandbox,
    /// The user forked the workspace manually (`enox workspace fork`).
    ManualFork,
}

impl SessionMode {
    /// The strongest attribution confidence this mode can justify on its own.
    pub fn default_confidence(self) -> Confidence {
        match self {
            SessionMode::Ambient => Confidence::Unknown,
            SessionMode::AmbientTriggered => Confidence::Session,
            SessionMode::ManagedProcess => Confidence::VerifiedProcess,
            SessionMode::ClaimedSession => Confidence::UserDeclared,
            SessionMode::Sandbox => Confidence::VerifiedWorkspace,
            SessionMode::ManualFork => Confidence::UserDeclared,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalChangeSession {
    pub session_id: String,
    pub circle_id: String,
    /// Snapshot id of the workspace when the session opened (S0).
    pub base_snapshot: String,
    pub mode: SessionMode,
    pub trigger_id: Option<String>,
    pub requested_agent: Option<String>,
    pub actor_id: Option<String>,
    pub actor_hint: Option<String>,
    pub confidence: Confidence,
    /// Agents that relayed this work, innermost last. Empty for a turn a
    /// person asked for directly. `actor_id` says who wrote the files; this
    /// says who asked, which for a delegated turn is a different answer.
    #[serde(default)]
    pub relay_path: Vec<String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl LocalChangeSession {
    pub fn start(circle_id: String, base_snapshot: String, mode: SessionMode) -> Self {
        Self {
            session_id: uuid::Uuid::new_v4().to_string(),
            circle_id,
            base_snapshot,
            mode,
            trigger_id: None,
            requested_agent: None,
            actor_id: None,
            actor_hint: None,
            confidence: mode.default_confidence(),
            relay_path: Vec::new(),
            started_at: chrono::Utc::now(),
            finished_at: None,
        }
    }

    pub fn finish(&mut self) {
        if self.finished_at.is_none() {
            self.finished_at = Some(chrono::Utc::now());
        }
    }

    pub fn is_open(&self) -> bool {
        self.finished_at.is_none()
    }

    /// Path of the single "current claimed session" record for a circle dir.
    /// One open claimed session per workspace keeps attribution unambiguous;
    /// concurrent actors are intentionally rejected to keep attribution
    /// unambiguous.
    pub fn claimed_path(circle_dir: &std::path::Path) -> std::path::PathBuf {
        circle_dir.join("claimed_session.json")
    }

    /// Persist this session as the circle's current claimed session.
    pub fn save_claimed(&self, circle_dir: &std::path::Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(circle_dir)?;
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(Self::claimed_path(circle_dir), json)?;
        Ok(())
    }

    /// Load the circle's current claimed session, if one is recorded.
    pub fn load_claimed(circle_dir: &std::path::Path) -> Option<Self> {
        let text = std::fs::read_to_string(Self::claimed_path(circle_dir)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Path of the managed-process session record consumed by the proposal
    /// engine. The record remains after process exit long enough for debounced
    /// filesystem events to be attributed correctly.
    pub fn managed_path(circle_dir: &std::path::Path) -> std::path::PathBuf {
        circle_dir.join("managed_session.json")
    }

    pub fn save_managed(&self, circle_dir: &std::path::Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(circle_dir)?;
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(Self::managed_path(circle_dir), json)?;
        Ok(())
    }

    pub fn load_managed(circle_dir: &std::path::Path) -> Option<Self> {
        let text = std::fs::read_to_string(Self::managed_path(circle_dir)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Drop a managed-session record left open by a previous daemon.
    ///
    /// The record is a lock: `driver::launch` refuses to start an agent while
    /// one is open. Nothing ever expires it, so a daemon that dies mid-run —
    /// killed, restarted, crashed — leaves a lock that blocks *every* future
    /// agent run in that Circle, permanently, with a message claiming the agent
    /// "is already running". It is not running; the process that ran it is gone.
    ///
    /// A managed agent is a child of the daemon, so once a new daemon starts,
    /// any session still marked open belongs to a process that no longer
    /// exists. Two daemons cannot share a Circle — the second fails to bind the
    /// port — so "open at startup" means "orphaned" with no ambiguity.
    ///
    /// Returns the agent named by the record it cleared, for logging.
    pub fn clear_orphaned_managed(circle_dir: &std::path::Path) -> Option<String> {
        let current = Self::load_managed(circle_dir)?;
        if !current.is_open() {
            return None;
        }
        std::fs::remove_file(Self::managed_path(circle_dir)).ok()?;
        Some(current.actor_id.unwrap_or_else(|| "?".to_string()))
    }

    pub fn clear_managed_if(circle_dir: &std::path::Path, session_id: &str) -> anyhow::Result<()> {
        let Some(current) = Self::load_managed(circle_dir) else {
            return Ok(());
        };
        if current.session_id == session_id {
            let path = Self::managed_path(circle_dir);
            if path.exists() {
                std::fs::remove_file(path)?;
            }
        }
        Ok(())
    }

    /// Whether filesystem activity at `observed_at` can belong to this run.
    /// A short tail covers watcher delivery that races with child-process exit.
    pub fn contains_activity_at(&self, observed_at: chrono::DateTime<chrono::Utc>) -> bool {
        if observed_at < self.started_at {
            return false;
        }
        self.finished_at
            .map(|finished| observed_at <= finished + chrono::Duration::seconds(5))
            .unwrap_or(true)
    }

    /// Remove the claimed-session record (called on `session finish`).
    pub fn clear_claimed(circle_dir: &std::path::Path) -> anyhow::Result<()> {
        let path = Self::claimed_path(circle_dir);
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    // TODO(M14): timeout handling for chat-triggered sessions that never
    // produce changes, and the concurrent-actor question for claimed sessions.
}

#[cfg(test)]
mod tests {
    use super::*;

    fn managed(dir: &std::path::Path, agent: &str, finished: bool) -> LocalChangeSession {
        let mut session =
            LocalChangeSession::start("c".into(), "base".into(), SessionMode::ManagedProcess);
        session.actor_id = Some(agent.into());
        if finished {
            session.finish();
        }
        session.save_managed(dir).unwrap();
        session
    }

    #[test]
    fn an_open_session_from_a_dead_daemon_is_cleared() {
        // The lock that blocked every mention in a Circle: a run killed
        // mid-flight leaves finished_at unset and nothing ever expires it.
        let dir = tempfile::tempdir().unwrap();
        managed(dir.path(), "suzent", false);
        assert_eq!(
            LocalChangeSession::clear_orphaned_managed(dir.path()),
            Some("suzent".to_string())
        );
        assert!(
            LocalChangeSession::load_managed(dir.path()).is_none(),
            "the orphaned lock must be gone, not merely reported"
        );
    }

    #[test]
    fn a_finished_session_is_left_alone() {
        // A closed record is history the proposal engine may still want to
        // attribute against; only an *open* one is a lock worth breaking.
        let dir = tempfile::tempdir().unwrap();
        let session = managed(dir.path(), "claude", true);
        assert_eq!(LocalChangeSession::clear_orphaned_managed(dir.path()), None);
        assert_eq!(
            LocalChangeSession::load_managed(dir.path())
                .unwrap()
                .session_id,
            session.session_id
        );
    }

    #[test]
    fn no_record_at_all_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(LocalChangeSession::clear_orphaned_managed(dir.path()), None);
    }

    #[test]
    fn clearing_an_orphan_lets_the_next_run_take_the_lock() {
        // The property that actually matters: after recovery the Circle is
        // usable again.
        let dir = tempfile::tempdir().unwrap();
        managed(dir.path(), "suzent", false);
        LocalChangeSession::clear_orphaned_managed(dir.path());
        let next = managed(dir.path(), "codex", false);
        let held = LocalChangeSession::load_managed(dir.path()).unwrap();
        assert_eq!(held.session_id, next.session_id);
        assert!(held.is_open());
    }

    #[test]
    fn start_finish_lifecycle() {
        let mut session = LocalChangeSession::start(
            "circle-1".into(),
            "snap-0".into(),
            SessionMode::ClaimedSession,
        );
        assert!(session.is_open());
        assert_eq!(session.confidence, Confidence::UserDeclared);
        session.finish();
        assert!(!session.is_open());
        let finished = session.finished_at;
        session.finish();
        assert_eq!(session.finished_at, finished, "finish is idempotent");
    }

    #[test]
    fn mode_confidence_mapping() {
        assert_eq!(
            SessionMode::Ambient.default_confidence(),
            Confidence::Unknown
        );
        assert_eq!(
            SessionMode::ManagedProcess.default_confidence(),
            Confidence::VerifiedProcess
        );
        assert_eq!(
            SessionMode::AmbientTriggered.default_confidence(),
            Confidence::Session
        );
    }

    #[test]
    fn managed_session_bounds_file_activity() {
        let mut session = LocalChangeSession::start(
            "circle-1".into(),
            "snap-0".into(),
            SessionMode::ManagedProcess,
        );
        let before = session.started_at - chrono::Duration::milliseconds(1);
        assert!(!session.contains_activity_at(before));
        assert!(session.contains_activity_at(session.started_at));
        session.finished_at = Some(session.started_at + chrono::Duration::seconds(2));
        assert!(session
            .contains_activity_at(session.finished_at.unwrap() + chrono::Duration::seconds(5)));
        assert!(!session
            .contains_activity_at(session.finished_at.unwrap() + chrono::Duration::seconds(6)));
    }

    #[test]
    fn managed_session_round_trips_until_consumed() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = LocalChangeSession::start(
            "circle-1".into(),
            "snap-0".into(),
            SessionMode::ManagedProcess,
        );
        session.actor_id = Some("codex".into());
        session.save_managed(dir.path()).unwrap();
        assert_eq!(
            LocalChangeSession::load_managed(dir.path())
                .unwrap()
                .actor_id
                .as_deref(),
            Some("codex")
        );
        LocalChangeSession::clear_managed_if(dir.path(), "another-session").unwrap();
        assert!(LocalChangeSession::load_managed(dir.path()).is_some());
        LocalChangeSession::clear_managed_if(dir.path(), &session.session_id).unwrap();
        assert!(LocalChangeSession::load_managed(dir.path()).is_none());
    }
}
