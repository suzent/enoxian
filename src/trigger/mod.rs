//! Circle-layer agent trigger protocol.
//!
//! A chat mention creates intent; the local daemon decides whether to act.
//!
//! ```text
//! circle layer:  agent_triggered event (replicated, signed, auditable)
//!                         |
//!                         v
//! local daemon:  allowlist check -> launch -> LocalChangeSession -> watcher
//!                         |
//!                         v
//! circle layer:  trigger status reply
//! ```
//!
//! The circle event carries only portable fields. Agent launch details
//! (binary, command template, working dir, timeouts) are daemon-local and
//! live in the registry (`registry`). The replicated control doc / event log
//! is the delivery channel; no webhook or extra HTTP surface is involved.

pub mod handler;
pub mod registry;

use serde::{Deserialize, Serialize};

/// Replicated circle event: a member requested that an agent do some work.
/// This is intent, not a guaranteed process identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTriggered {
    pub trigger_id: String,
    pub circle_id: String,
    /// Routing hint only — never a security boundary. The target daemon's
    /// local allowlist is the gate.
    pub requested_agent: String,
    /// The text after the mention.
    pub task_text: String,
    /// Peer/member id of the requester.
    pub requested_by: String,
    /// Chat message that produced this trigger.
    pub message_id: String,
    pub workspace_hint: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl AgentTriggered {
    pub fn new(
        circle_id: String,
        requested_agent: String,
        task_text: String,
        requested_by: String,
        message_id: String,
    ) -> Self {
        Self {
            trigger_id: uuid::Uuid::new_v4().to_string(),
            circle_id,
            requested_agent,
            task_text,
            requested_by,
            message_id,
            workspace_hint: None,
            created_at: chrono::Utc::now(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerStatus {
    /// Target device/agent saw the trigger.
    Delivered,
    /// A local change session was opened.
    Started,
    /// No matching agent in the local allowlist.
    Ignored,
    /// No response before timeout.
    Expired,
    /// A proposal was created from the session.
    Completed,
    /// Launch or runtime error.
    Failed,
}

/// Replicated back to the circle so the chat room can show trigger feedback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerStatusReply {
    pub trigger_id: String,
    pub status: TriggerStatus,
    pub detail: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Parses a leading agent mention from a chat message.
///
/// ```text
/// "@codex fix the sync docs"        -> ("codex", "fix the sync docs")
/// "@alice/claude review the layer"  -> ("alice/claude", "review the layer")
/// ```
pub fn parse_mention(text: &str) -> Option<(&str, &str)> {
    let rest = text.trim_start().strip_prefix('@')?;
    let (agent, task) = rest.split_once(char::is_whitespace)?;
    let task = task.trim();
    if agent.is_empty() || task.is_empty() {
        return None;
    }
    Some((agent, task))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_mention() {
        assert_eq!(
            parse_mention("@codex fix the sync docs"),
            Some(("codex", "fix the sync docs"))
        );
    }

    #[test]
    fn parses_member_scoped_mention() {
        assert_eq!(
            parse_mention("@alice/claude review the proposal layer"),
            Some(("alice/claude", "review the proposal layer"))
        );
    }

    #[test]
    fn rejects_non_mentions() {
        assert_eq!(parse_mention("plain message"), None);
        assert_eq!(parse_mention("@agent_without_task"), None);
        assert_eq!(parse_mention("@ task without agent"), None);
        assert_eq!(parse_mention("email me at foo@bar.com"), None);
    }
}
