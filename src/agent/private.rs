//! Private delivery: a message for one agent on this device that never enters
//! the Circle.
//!
//! Everything else that reaches an agent arrives as chat, which every device
//! replicates. Some input belongs to one machine only — a fact its person knows,
//! or a requirement that changed and was told to this device first — and the
//! agent should hear it the way it would from someone at the keyboard, without
//! the room hearing it too.
//!
//! A private message goes straight into the local execution inbox (see
//! [`super::inbox`]) and is never written to the control doc. Its turn then
//! leaves nothing in the Circle that names it:
//!
//! - no chat activity (seen / working / skipped) is published for it;
//! - no execution receipt is published for it;
//! - the agent's reply is kept on the local inbox entry, not posted to chat;
//! - a failure is not announced in the room;
//! - proposals from the turn carry no trigger id.
//!
//! What the agent then does with what it heard is up to the agent: it can
//! still `enox say` or edit shared files like on any other turn.

use crate::control::{Author, ChatMessage};
use crate::state::AppState;

/// Prefix on a private message's id. Chat ids are bare UUIDs, so this cannot
/// collide with one, and it is how every Circle-facing path recognises a turn
/// it must keep to itself.
pub const ID_PREFIX: &str = "private-";

/// The mention key a private request is admitted under. Not a mention anyone
/// can type: it only has to keep the inbox's `(message, agent)` key unique.
pub const MENTION_KEY: &str = "~private";

/// Is this message id one that must not be published?
pub fn is_private(message_id: &str) -> bool {
    message_id.starts_with(ID_PREFIX)
}

/// A request for `agent` carrying `text`, as said by `from` (the local member
/// by default).
pub fn request(
    state: &AppState,
    agent: &str,
    text: &str,
    from: Option<&str>,
) -> super::inbox::Request {
    let message = ChatMessage {
        thread_root: None,
        id: format!("{ID_PREFIX}{}", uuid::Uuid::new_v4()),
        agent_id: from
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .unwrap_or(&state.agent_id)
            .to_string(),
        text: text.to_string(),
        mentions: Vec::new(),
        ts: chrono::Utc::now().timestamp(),
        peer_id: state.peer_id.clone(),
        attachments: Vec::new(),
        relay: None,
        author: Author::Human,
        reply_to: None,
    };
    super::inbox::Request {
        agent: agent.to_string(),
        mention_key: MENTION_KEY.to_string(),
        task: text.trim().to_string(),
        message,
        relay: None,
        implicit: false,
        ambient: false,
        co_listeners: Vec::new(),
        warden: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_prefixed_ids_are_private() {
        assert!(is_private("private-0b1c"));
        assert!(!is_private("0b1c2d3e-4f50-6172-8394-a5b6c7d8e9f0"));
        assert!(!is_private("~private"));
    }
}
