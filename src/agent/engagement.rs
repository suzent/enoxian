//! Follow-up routing: a reply to an agent should not need a mention.
//!
//! See `docs/development/engagement.md` §1.1. After an agent answers you,
//! re-typing `@claude` says nothing the room does not already know, so for a
//! short window your next message routes back to the same agent on the same
//! machine.
//!
//! ## Why this is derived, not stored
//!
//! The spec describes an in-memory map on the device that ran the agent. That
//! works for one machine and breaks across several: the composer that must show
//! "replying to @claude" runs on the *speaker's* device, which would know
//! nothing about a map held on the agent's device.
//!
//! So the engagement is *computed from the transcript* instead, by a rule every
//! device can evaluate identically:
//!
//! > the most recent agent reply to a cascade **you** started, within the
//! > window, that you have not dismissed.
//!
//! Both halves the spec asks for fall out of data already on the wire. "Who was
//! the agent replying to" is `relay.root_peer` — the human at the root of the
//! cascade, which delegation already records. "Which machine must the follow-up
//! wake" is that reply's `peer_id`, the device that actually ran it, which is
//! exactly the scope the spec wants carried forward.
//!
//! The one thing not derivable is dismissal, because it is an intention rather
//! than an event. That lives in the control doc (`ENGAGEMENT_EXITS_KEY`), for
//! the same reason the delegation stop does: the device that would route the
//! follow-up is usually not the device whose user pressed Esc.

use crate::control::{ChatMessage, Relay};

/// Where a mention-less message should be routed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Engagement {
    /// The agent to wake.
    pub agent: String,
    /// The device that ran it, and must run the follow-up. A follow-up has to
    /// wake the same machine, not whichever device happens to configure an
    /// agent by that name.
    pub peer_id: String,
    /// The agent reply this was resolved from.
    pub message_id: String,
}

/// Is this message eligible for implicit routing at all?
///
/// Only a human's message with no agent-level mention. An explicit mention is
/// addressing and always wins; an agent's own reply must never be treated as a
/// follow-up, or agents would hold conversations with each other outside the
/// relay budget (§3.3).
pub fn is_followup_candidate(msg: &ChatMessage, mention_targets_agent: bool) -> bool {
    if mention_targets_agent {
        return false;
    }
    if msg.agent_id == "system" {
        return false;
    }
    // An agent reply carries a relay whose path names the agent that posted it.
    match &msg.relay {
        Some(relay) => relay.path.is_empty(),
        None => true,
    }
}

/// Resolve the engagement for `speaker` from the transcript.
///
/// `history` must be in chronological order and end before the message being
/// routed. `now` is that message's timestamp.
/// What a speaker dismissed: when, and which reply was on screen at the time.
///
/// The timestamp alone is not enough. Chat timestamps have one-second
/// resolution, so a dismissal and the reply that re-arms the window can share
/// one — and comparing with `<=` would then swallow a reply that actually came
/// after. Naming the dismissed message removes the tie: anything *older* is
/// dismissed by time, that exact message is dismissed by identity, and anything
/// else is new.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dismissal {
    pub at: i64,
    pub message_id: String,
}

impl Dismissal {
    fn covers(&self, msg: &ChatMessage) -> bool {
        msg.id == self.message_id || msg.ts < self.at
    }
}

pub fn resolve(
    history: &[ChatMessage],
    speaker_peer: &str,
    now: i64,
    window_secs: i64,
    dismissed: Option<&Dismissal>,
) -> Option<Engagement> {
    if window_secs <= 0 || speaker_peer.is_empty() {
        return None;
    }
    for msg in history.iter().rev() {
        if now - msg.ts > window_secs {
            // Everything older is out of the window too.
            return None;
        }
        let Some(relay) = &msg.relay else { continue };
        let Some(agent) = relay.path.last() else {
            continue;
        };
        if relay.root_peer != speaker_peer {
            // Someone else's conversation. Deliberately not a stopping
            // condition: two people can hold separate conversations with
            // separate agents in one room, and a third person's chatter
            // disturbs neither.
            continue;
        }
        if dismissed.is_some_and(|d| d.covers(msg)) {
            // The speaker pressed Esc on this reply.
            return None;
        }
        if msg.peer_id.is_empty() {
            // Cannot tell which machine to wake; refuse rather than guess.
            return None;
        }
        return Some(Engagement {
            agent: agent.clone(),
            peer_id: msg.peer_id.clone(),
            message_id: msg.id.clone(),
        });
    }
    None
}

/// The dedup key for an implicitly routed turn.
///
/// An implicit route has no mention string to key on. The `@` prefix is what
/// keeps it distinct from a real mention of the same agent on the same message
/// — stored mention bodies never carry one (`mention::extract` strips it), so
/// an explicit mention and a follow-up can never both fire for one message.
pub fn dedup_key(agent: &str) -> String {
    format!("@{agent}")
}

/// The relay a follow-up turn inherits.
///
/// A follow-up is a human message, so it mints a fresh budget like any other —
/// being routed implicitly does not make it an agent-driven turn. Humans are
/// not rate-limited by their agents' spending (§3.3).
pub fn followup_relay(message_id: &str, speaker_peer: &str) -> Relay {
    super::relay::mint(message_id, speaker_peer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn human(id: &str, peer: &str, ts: i64) -> ChatMessage {
        ChatMessage {
            id: id.into(),
            agent_id: "suzy".into(),
            text: "hello".into(),
            mentions: vec![],
            ts,
            peer_id: peer.into(),
            attachments: vec![],
            relay: Some(crate::agent::relay::mint(id, peer)),
        }
    }

    fn agent_reply(id: &str, agent: &str, ran_on: &str, root_peer: &str, ts: i64) -> ChatMessage {
        let parent = crate::agent::relay::mint("root", root_peer);
        ChatMessage {
            id: id.into(),
            agent_id: agent.into(),
            text: "sure".into(),
            mentions: vec![],
            ts,
            peer_id: ran_on.into(),
            attachments: vec![],
            relay: Some(crate::agent::relay::extend(&parent, agent)),
        }
    }

    #[test]
    fn a_reply_to_me_routes_my_next_message_back_to_it() {
        let h = vec![agent_reply("a1", "claude", "dev-mac", "peer-suzy", 100)];
        let e = resolve(&h, "peer-suzy", 110, 180, None).unwrap();
        assert_eq!(e.agent, "claude");
        assert_eq!(e.peer_id, "dev-mac", "the follow-up wakes the same machine");
    }

    #[test]
    fn someone_elses_conversation_is_not_mine_to_continue() {
        let h = vec![agent_reply("a1", "claude", "dev-mac", "peer-bob", 100)];
        assert!(resolve(&h, "peer-suzy", 110, 180, None).is_none());
    }

    #[test]
    fn a_third_partys_chatter_does_not_end_my_engagement() {
        // Keyed per speaker: bob talking to codex must not break suzy's thread.
        let h = vec![
            agent_reply("a1", "claude", "dev-mac", "peer-suzy", 100),
            human("m2", "peer-bob", 105),
            agent_reply("a2", "codex", "dev-air", "peer-bob", 106),
        ];
        let e = resolve(&h, "peer-suzy", 110, 180, None).unwrap();
        assert_eq!(e.agent, "claude");
    }

    #[test]
    fn the_most_recent_reply_wins_when_i_have_talked_to_two_agents() {
        let h = vec![
            agent_reply("a1", "claude", "dev-mac", "peer-suzy", 100),
            agent_reply("a2", "codex", "dev-air", "peer-suzy", 105),
        ];
        assert_eq!(
            resolve(&h, "peer-suzy", 110, 180, None).unwrap().agent,
            "codex"
        );
    }

    #[test]
    fn the_window_expires() {
        let h = vec![agent_reply("a1", "claude", "dev-mac", "peer-suzy", 100)];
        assert!(resolve(&h, "peer-suzy", 100 + 180, 180, None).is_some());
        assert!(resolve(&h, "peer-suzy", 100 + 181, 180, None).is_none());
    }

    #[test]
    fn a_zero_window_disables_the_feature() {
        let h = vec![agent_reply("a1", "claude", "dev-mac", "peer-suzy", 100)];
        assert!(resolve(&h, "peer-suzy", 101, 0, None).is_none());
    }

    #[test]
    fn dismissing_ends_it_and_a_later_reply_starts_a_new_one() {
        let h = vec![agent_reply("a1", "claude", "dev-mac", "peer-suzy", 100)];
        assert!(resolve(
            &h,
            "peer-suzy",
            110,
            180,
            Some(&Dismissal {
                at: 105,
                message_id: "a1".into()
            })
        )
        .is_none());
        // A dismissal does not poison the future: an agent that replies after
        // it opens a fresh engagement.
        let h2 = vec![
            agent_reply("a1", "claude", "dev-mac", "peer-suzy", 100),
            agent_reply("a2", "claude", "dev-mac", "peer-suzy", 120),
        ];
        assert!(resolve(
            &h2,
            "peer-suzy",
            130,
            180,
            Some(&Dismissal {
                at: 105,
                message_id: "a1".into()
            })
        )
        .is_some());
    }

    #[test]
    fn a_human_message_is_not_a_reply_to_follow_up_on() {
        let h = vec![human("m1", "peer-suzy", 100)];
        assert!(resolve(&h, "peer-suzy", 110, 180, None).is_none());
    }

    #[test]
    fn an_explicit_mention_is_never_implicitly_routed() {
        let mut m = human("m1", "peer-suzy", 100);
        m.mentions = vec!["claude".into()];
        assert!(!is_followup_candidate(&m, true), "addressing always wins");
        assert!(is_followup_candidate(&m, false));
    }

    #[test]
    fn an_agent_reply_is_never_a_follow_up() {
        // Otherwise agents would converse outside the relay budget.
        let reply = agent_reply("a1", "claude", "dev-mac", "peer-suzy", 100);
        assert!(!is_followup_candidate(&reply, false));
    }

    #[test]
    fn a_system_post_is_never_a_follow_up() {
        let mut m = human("m1", "peer-suzy", 100);
        m.agent_id = "system".into();
        m.relay = None;
        assert!(!is_followup_candidate(&m, false));
    }

    #[test]
    fn a_message_from_an_older_peer_can_still_follow_up() {
        let mut m = human("m1", "peer-suzy", 100);
        m.relay = None; // predates the relay field
        assert!(is_followup_candidate(&m, false));
    }

    #[test]
    fn the_dedup_key_cannot_collide_with_a_real_mention() {
        // Stored mention bodies never carry '@'.
        assert_eq!(dedup_key("claude"), "@claude");
        assert!(!crate::agent::mention::extract("@claude hi").contains(&"@claude".to_string()));
    }
}
