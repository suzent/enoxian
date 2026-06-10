pub mod arbitration;
pub mod fs_lock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const LOCK_LOG_KEY: &str = "lock_log";
pub const TASKS_KEY: &str = "tasks";
pub const PRESENCE_KEY: &str = "presence";
pub const MEMBER_LIST_KEY: &str = "member_list";
pub const CHAT_KEY: &str = "chat";

// ── MLS delivery-service keys (M11) ───────────────────────────────────────
// Stored in the __control__ Yjs map; replicated to all peers via CRDT sync.

/// Map[peer_id → hex(KeyPackage TLS bytes)] — each peer publishes on daemon start.
pub const MLS_KEY_PACKAGES_KEY: &str = "mls_key_packages";
/// Map[peer_id → hex(Welcome TLS bytes)] — admin stores after `member add`.
pub const MLS_WELCOMES_KEY: &str = "mls_welcomes";
/// Array[MlsCommitEntry] — every Commit stored so offline members can catch up.
pub const MLS_COMMITS_KEY: &str = "mls_commits";
/// Map[peer_id → PendingEntry JSON] — peers waiting for admin approval.
pub const MLS_PENDING_KEY: &str = "mls_pending";
/// Map[peer_id → OwnerClaim JSON] — self-signed owner name claims.
pub const MLS_OWNER_CLAIMS_KEY: &str = "mls_owner_claims";
/// Map[peer_id → RFC-3339 timestamp] — peers that have been explicitly removed.
/// Used as a sync-level gate: removed peers are rejected before any CRDT data
/// is exchanged, even during the brief window before PSK rotation completes.
pub const MLS_REMOVED_KEY: &str = "mls_removed";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnerClaim {
    pub owner: String,
    /// hex(sign(peer_keypair, "owner:{owner}"))
    pub sig: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingEntry {
    pub peer_id: String,
    pub owner: String,
    pub agent_id: String,
    /// Human-readable device label (e.g. "macbook-pro").
    #[serde(default)]
    pub device_label: String,
    /// Agent names registered on this device (e.g. ["human", "claude-code"]).
    #[serde(default)]
    pub agents: Vec<String>,
    /// hex(sign(peer_keypair, "owner:{owner}"))
    pub owner_sig: String,
    pub requested_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MlsCommitEntry {
    pub epoch: u64,
    pub data_hex: String,
    pub sender_peer_id: String,
    pub ratchet_tree_hex: String,
}

// ── Lock ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockEntry {
    pub entry_id: String,
    pub agent_id: String,
    pub path: String,
    pub action: LockAction,
    pub ts: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LockAction {
    Acquire,
    Release,
}

// ── Task ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub task_id: String,
    pub title: String,
    pub description: Option<String>,
    pub status: TaskStatus,
    pub created_by: String,
    pub claimed_by: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Open,
    Claimed,
    Done,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open => write!(f, "open"),
            Self::Claimed => write!(f, "claimed"),
            Self::Done => write!(f, "done"),
        }
    }
}

// ── Presence ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Presence {
    pub agent_id: String,
    pub status: AgentStatus,
    pub last_seen: DateTime<Utc>,
    pub current_file: Option<String>,
    /// The peer_id of the device this agent is running on. Links presence to member entry.
    #[serde(default)]
    pub peer_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Online,
    Idle,
    Offline,
}

// ── Members ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MemberRole { Admin, Member }

impl std::fmt::Display for MemberRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Admin => write!(f, "admin"),
            Self::Member => write!(f, "member"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberEntry {
    pub peer_id: String,
    /// Human owner — groups machines belonging to the same person (e.g. "alice").
    /// Multiple peer_ids with the same owner = same user on different machines.
    pub owner: String,
    /// Primary agent label for this device (legacy display name, e.g. "alice-Kj4R").
    pub agent_id: String,
    /// Human-readable device label (e.g. "macbook-pro"). Shown in grouped UI.
    #[serde(default)]
    pub device_label: String,
    /// Agent names enrolled from this device (e.g. ["human", "claude-code"]).
    /// Pure labels — no separate keys. File edits are attributed to the device (peer_id).
    #[serde(default)]
    pub agents: Vec<String>,
    pub role: MemberRole,
    pub added_at: DateTime<Utc>,
    /// Hex-encoded Ed25519 admin signature of "add:{peer_id}:{role}"
    pub signature: String,
}

// ── Chat ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub agent_id: String,
    pub text: String,
    pub mentions: Vec<String>,
    pub ts: i64, // Unix timestamp seconds
}

// ── Events ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CircleEvent {
    FileUpdated { path: String },
    FileDeleted { path: String },
    LockAcquired { path: String, agent_id: String },
    LockReleased { path: String, agent_id: String },
    TaskCreated { task_id: String },
    TaskClaimed { task_id: String, agent_id: String },
    TaskDone { task_id: String },
    PresenceChanged { agent_id: String },
    MemberAdded { peer_id: String },
    MemberRemoved { peer_id: String },
    /// A chat message was posted to the circle.
    MessagePosted { message: ChatMessage },
    /// A message mentioned a specific agent — the agent's wake signal.
    AgentMentioned { agent_id: String, message: ChatMessage },
    /// The proposal engine captured a workspace change (M14).
    ProposalCreated { proposal_id: String },
    /// A proposal's status changed (accepted / rejected / reverted).
    ProposalUpdated { proposal_id: String, status: String },
}
