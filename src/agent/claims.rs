//! Who is already handling a message, across every device in the Circle.
//!
//! A claim is not a new kind of state. An ACP turn has always published a
//! [`ChatActivityKind::Working`] activity for the message it is answering and
//! renewed it every fifteen seconds, and that activity is replicated to every
//! peer — so "someone is on this" was already on the wire. Nothing read it for
//! a decision; it only drove the composer's "working" indicator. This module is
//! the reading.
//!
//! An *explicit* claim — a CLI agent or a person saying "I have this one" — is
//! the same kind of activity with a longer lifetime, stored under its own
//! `claim:` key so releasing it can never cancel a running turn's heartbeat.
//! Either way it expires by itself, which is what stops a crashed claimer from
//! holding a message forever.

use crate::control::{ChatActivity, ChatActivityKind};
use std::collections::HashMap;

/// How long an explicit claim lasts when the claimer does not say.
pub const DEFAULT_CLAIM_SECS: i64 = 600;

/// The longest an explicit claim may be held before it has to be renewed.
///
/// Bounded because a claim silences every ambient listener in the room for its
/// duration. An hour is long enough for real work and short enough that a
/// forgotten claim does not quietly mute a Circle for the rest of the day.
pub const MAX_CLAIM_SECS: i64 = 3600;

/// One live claim on one message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    pub message_id: String,
    pub agent: String,
    pub peer_id: String,
    pub expires_at: i64,
    /// True for a claim someone made on purpose; false for a turn that is
    /// simply running and advertising that it is.
    pub explicit: bool,
}

/// The id an explicit claim is stored under.
pub fn claim_activity_id(message_id: &str, agent: &str, peer_id: &str) -> String {
    format!("claim:{message_id}:{agent}:{peer_id}")
}

/// Live claims, grouped by the message they are on.
pub fn live_claims(activities: &[ChatActivity]) -> HashMap<String, Vec<Claim>> {
    let mut out: HashMap<String, Vec<Claim>> = HashMap::new();
    for activity in activities {
        if activity.kind != ChatActivityKind::Working {
            continue;
        }
        let Some(message_id) = activity.message_id.clone() else {
            continue;
        };
        out.entry(message_id.clone()).or_default().push(Claim {
            message_id,
            agent: activity.actor_id.clone(),
            peer_id: activity.peer_id.clone(),
            expires_at: activity.expires_at,
            explicit: activity.activity_id.starts_with("claim:"),
        });
    }
    out
}

/// Does someone else hold this message, from where this device is standing?
///
/// `mine` is every agent this device itself offered the message to, on this
/// device (`my_peer`). A claim by one of those is not a competitor — it is the
/// plan — and treating it as one breaks `ambient_responders = 2` outright: two
/// listeners are admitted together, and whichever started first would cancel
/// the other. Anyone else's claim blocks, whether it comes from another device
/// or from an agent on this one that was never offered the message, such as a
/// CLI agent that pulled it from its inbox.
pub fn blocking<'a>(
    claims: &'a HashMap<String, Vec<Claim>>,
    message_id: &str,
    my_peer: &str,
    mine: &[String],
) -> Option<&'a Claim> {
    claims.get(message_id)?.iter().find(|claim| {
        let ours = claim.peer_id == my_peer
            && mine
                .iter()
                .any(|agent| agent.eq_ignore_ascii_case(&claim.agent));
        !ours
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn working(id: &str, message: &str, agent: &str, peer: &str) -> ChatActivity {
        ChatActivity {
            activity_id: id.into(),
            actor_id: agent.into(),
            peer_id: peer.into(),
            kind: ChatActivityKind::Working,
            detail: None,
            message_id: Some(message.into()),
            updated_at: 0,
            expires_at: 100,
        }
    }

    fn mine(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_turn_on_another_device_is_a_claim() {
        let claims = live_claims(&[working("agent:m:claude:B", "m", "claude", "B")]);
        let found = blocking(&claims, "m", "A", &mine(&["claude"])).unwrap();
        assert_eq!(
            found.peer_id, "B",
            "same name, different machine: a competitor"
        );
        assert!(!found.explicit);
    }

    #[test]
    fn a_co_listener_on_this_device_is_not_a_competitor() {
        // Without this, `ambient_responders = 2` cancels itself: both
        // listeners are admitted together and the first to start would stop
        // the second.
        let claims = live_claims(&[working("agent:m:suzent:A", "m", "suzent", "A")]);
        assert_eq!(
            blocking(&claims, "m", "A", &mine(&["claude", "suzent"])),
            None
        );
    }

    #[test]
    fn an_agent_never_blocks_itself() {
        // A requeued turn must not be stopped by its own heartbeat from the
        // attempt a restart just killed.
        let claims = live_claims(&[working("agent:m:claude:A", "m", "claude", "A")]);
        assert_eq!(blocking(&claims, "m", "A", &mine(&["claude"])), None);
    }

    #[test]
    fn a_cli_agent_on_this_device_that_pulled_the_message_blocks() {
        // The case pull exists for: an agent that was never offered the
        // message takes it, and the ambient listeners here leave it alone.
        let claims = live_claims(&[working("claim:m:reviewer:A", "m", "reviewer", "A")]);
        let found = blocking(&claims, "m", "A", &mine(&["claude"])).unwrap();
        assert!(found.explicit);
        assert_eq!(found.agent, "reviewer");
    }

    #[test]
    fn only_working_activities_on_a_message_are_claims() {
        let mut typing = working("typing:suzy", "m", "suzy", "A");
        typing.kind = ChatActivityKind::Typing;
        let mut seen = working("agent:m:claude:B", "m", "claude", "B");
        seen.kind = ChatActivityKind::Seen;
        let mut unanchored = working("agent:x:claude:B", "m", "claude", "B");
        unanchored.message_id = None;
        assert!(live_claims(&[typing, seen, unanchored]).is_empty());
    }
}
