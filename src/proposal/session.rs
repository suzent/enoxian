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

    // TODO(M14): `enox session start --actor X` / `enox session finish` CLI,
    // session persistence under the daemon state dir, and timeout handling
    // for chat-triggered sessions that never produce changes.
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(SessionMode::Ambient.default_confidence(), Confidence::Unknown);
        assert_eq!(
            SessionMode::ManagedProcess.default_confidence(),
            Confidence::VerifiedProcess
        );
        assert_eq!(
            SessionMode::AmbientTriggered.default_confidence(),
            Confidence::Session
        );
    }
}
