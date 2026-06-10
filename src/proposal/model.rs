//! Proposal data model. See `docs/plan/agent-workspaces.md`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Pending,
    Accepted,
    Synced,
    Conflicted,
    Rejected,
    Reverted,
}

/// How the change session that produced this proposal was opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalSource {
    Ambient,
    ChatTrigger,
    ManagedProcess,
    ClaimedSession,
    Sandbox,
    ManualFork,
}

/// How strongly the actor attribution is backed. Hints are routing metadata,
/// never security facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    VerifiedProcess,
    VerifiedWorkspace,
    UserDeclared,
    Session,
    Inferred,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    pub id: String,
    pub circle_id: String,
    /// Snapshot id of the workspace when the change session started (S0).
    pub base_snapshot: String,
    /// Snapshot id of the dirty result (S1).
    pub result_snapshot: String,
    pub changed_paths: Vec<String>,
    pub status: ProposalStatus,
    pub source: ProposalSource,
    pub actor_id: Option<String>,
    pub actor_hint: Option<String>,
    pub confidence: Confidence,
    pub trigger_id: Option<String>,
    pub session_id: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl Proposal {
    /// An unattributed ambient proposal. Unknown local edits are normal in an
    /// agent-agnostic system; they must not be discarded or auto-merged just
    /// because attribution is missing.
    pub fn ambient(
        circle_id: String,
        base_snapshot: String,
        result_snapshot: String,
        changed_paths: Vec<String>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            circle_id,
            base_snapshot,
            result_snapshot,
            changed_paths,
            status: ProposalStatus::Pending,
            source: ProposalSource::Ambient,
            actor_id: None,
            actor_hint: None,
            confidence: Confidence::Unknown,
            trigger_id: None,
            session_id: None,
            created_at: chrono::Utc::now(),
        }
    }
}
