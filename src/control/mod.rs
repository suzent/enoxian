pub mod arbitration;
pub mod fs_lock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const LOCK_LOG_KEY: &str = "lock_log";
pub const TASKS_KEY: &str = "tasks";
pub const PRESENCE_KEY: &str = "presence";
pub const MEMBER_LIST_KEY: &str = "member_list";
pub const CHAT_KEY: &str = "chat";

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
    pub agent_id: String,
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
}
