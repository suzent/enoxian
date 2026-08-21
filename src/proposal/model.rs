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
    /// Live edits through the daemon's own interactive surfaces (browser
    /// editor, P2P CRDT sync, UI file operations). Auto-accepted: recorded
    /// for history and revert, never held for review.
    Interactive,
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
    /// Peer ID of the device whose daemon captured this proposal.
    #[serde(default)]
    pub origin_peer_id: String,
    /// Human-readable label of that device (e.g. "my-laptop").
    #[serde(default)]
    pub origin_device: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// When the status last changed. Drives the cross-device conflict rule
    /// (see `resolve_status` / the proposal pull protocol). Legacy records
    /// without this field fall back to `created_at` via `effective_updated_at`.
    #[serde(default)]
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl Proposal {
    /// An unattributed ambient proposal. Ambient edits already happened in the
    /// live workspace, so their proposal is accepted history by default. The
    /// before/after snapshots retain review and revert without implying that
    /// acceptance gates the filesystem write.
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
            status: ProposalStatus::Accepted,
            source: ProposalSource::Ambient,
            actor_id: None,
            actor_hint: None,
            confidence: Confidence::Unknown,
            trigger_id: None,
            session_id: None,
            origin_peer_id: String::new(),
            origin_device: String::new(),
            created_at: chrono::Utc::now(),
            updated_at: None,
        }
    }

    /// The effective last-modified time: `updated_at` if set, else `created_at`.
    /// Lets legacy records (no `updated_at`) participate in conflict resolution.
    pub fn effective_updated_at(&self) -> chrono::DateTime<chrono::Utc> {
        self.updated_at.unwrap_or(self.created_at)
    }

    /// Set the status and stamp `updated_at`. Use this rather than assigning
    /// `status` directly so the conflict-resolution timestamp stays accurate.
    pub fn set_status(&mut self, status: ProposalStatus) {
        self.status = status;
        self.updated_at = Some(chrono::Utc::now());
    }

    /// A compact fingerprint of the mutable state, used by the pull protocol to
    /// detect when a peer's copy of this proposal has diverged (only the status
    /// changes after creation; the result snapshot id changes iff content does).
    /// Two peers agree on a proposal iff (id, fingerprint) match.
    pub fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (self.status as u8).hash(&mut h);
        self.result_snapshot.hash(&mut h);
        h.finish()
    }
}

impl ProposalStatus {
    /// Deterministic precedence for resolving concurrent status changes to the
    /// same proposal across devices. Higher wins; ties break by
    /// `effective_updated_at`. A terminal decision beats a pending one, and an
    /// explicit undo (reverted) beats the accept it undid.
    pub fn rank(self) -> u8 {
        match self {
            ProposalStatus::Pending => 0,
            ProposalStatus::Conflicted => 1,
            ProposalStatus::Accepted => 2,
            ProposalStatus::Synced => 2,
            ProposalStatus::Rejected => 3,
            ProposalStatus::Reverted => 4,
        }
    }
}

/// Pick the winning proposal record when two devices hold the same id with
/// different status. Greater `(status rank, effective_updated_at)` wins;
/// returns true if `incoming` should replace `local`. Deterministic across
/// peers regardless of clock skew on the rank component.
pub fn incoming_status_wins(local: &Proposal, incoming: &Proposal) -> bool {
    let l = (local.status.rank(), local.effective_updated_at());
    let r = (incoming.status.rank(), incoming.effective_updated_at());
    r > l
}
