//! Ambient engagement: letting an agent read the room and volunteer.
//!
//! See `docs/development/engagement.md` §2. An agent configured
//! `engagement = "ambient"` is offered every human message in the Circle and
//! may answer or decline. Nothing about it is addressed, which is what makes it
//! both the interesting idea and the expensive one.
//!
//! Participation is bounded by these gates and the shared execution queue:
//!
//! - **Human-authored only** (§2.1). An unaddressed agent's output can never
//!   become another unaddressed agent's input, so there is no fixpoint to chase.
//! - **A heuristic gate** (§2.5) that costs nothing, skipping the traffic that
//!   dominates the volume before any model is asked.
//! - **Shared capacity**. Every enabled listener can be offered a message,
//!   but device concurrency and per-agent/Circle queue bounds still apply.

use crate::control::{Author, ChatMessage};

/// Messages shorter than this are not worth a model turn.
///
/// "ok", "thanks", "lol" and "👍" are the bulk of a casual room and the least
/// likely to need an agent. Crude on purpose: it costs nothing and removes the
/// traffic that dominates the volume.
const MIN_AMBIENT_CHARS: usize = 24;

/// An agent that spoke within this many seconds is left alone.
///
/// It has just had its say; volunteering again immediately is the pile-on the
/// spec warns about, and the room reads as the agent talking to itself.
const AMBIENT_QUIET_SECS: i64 = 30;

/// The reply that means "nothing to add" (§2.3).
const PASS_TOKEN: &str = "PASS";

/// Did the agent decline rather than answer?
///
/// Distinguishing this from a crashed adapter is the whole point: an empty
/// reply already posts nothing, so without a convention "considered and passed"
/// and "never ran" look identical, and users stop trusting the Circle.
pub fn is_pass(reply: &str) -> bool {
    reply.trim().eq_ignore_ascii_case(PASS_TOKEN)
}

/// Should this message be offered to unaddressed agents at all?
///
/// The cheap checks only — no model, no cost. Returns the reason to skip, for
/// logging, or `None` to proceed.
pub fn skip_reason(msg: &ChatMessage, mentions_an_agent: bool) -> Option<&'static str> {
    if msg.author != Author::Human {
        // §2.1. The rule that keeps ambient from oscillating.
        return Some("not human-authored");
    }
    if mentions_an_agent {
        // Someone was addressed; that is a turn, not idle chat.
        return Some("addressed to an agent");
    }
    if msg.text.trim().chars().count() < MIN_AMBIENT_CHARS && msg.attachments.is_empty() {
        return Some("too short to be worth a turn");
    }
    None
}

/// Has this agent spoken too recently to volunteer again?
///
/// `history` is chronological.
pub fn spoke_recently(history: &[ChatMessage], agent: &str, now: i64) -> bool {
    history.iter().rev().any(|m| {
        now - m.ts <= AMBIENT_QUIET_SECS
            && m.author == Author::Agent
            && m.agent_id.eq_ignore_ascii_case(agent)
    })
}

/// The instruction appended for an unaddressed turn.
///
/// States the PASS convention explicitly, and that the turn is conversational:
/// an agent nobody asked to do anything should not be writing files (§2.4).
pub fn ambient_instruction() -> &'static str {
    "You were not addressed. You are seeing this because you are a participant in this room. \
     If you have nothing worth adding, reply with exactly PASS and nothing else. Do not explain \
     why you are passing. This is a conversational turn, not a work order: do not change files. \
     If something needs doing, say so and let someone ask for it."
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(text: &str, author: Author, ts: i64, agent: &str) -> ChatMessage {
        ChatMessage {
            thread_root: None,
            id: "m".into(),
            agent_id: agent.into(),
            text: text.into(),
            mentions: vec![],
            ts,
            peer_id: "p".into(),
            attachments: vec![],
            relay: None,
            author,
            reply_to: None,
        }
    }

    #[test]
    fn an_agent_reply_never_offers_an_ambient_turn() {
        // The property that makes the whole thing terminate.
        let m = msg(
            "a long enough sentence to pass the floor",
            Author::Agent,
            0,
            "claude",
        );
        assert_eq!(skip_reason(&m, false), Some("not human-authored"));
    }

    #[test]
    fn a_system_post_never_offers_one_either() {
        let m = msg(
            "a long enough sentence to pass the floor",
            Author::System,
            0,
            "system",
        );
        assert!(skip_reason(&m, false).is_some());
    }

    #[test]
    fn an_addressed_message_is_a_turn_not_idle_chat() {
        let m = msg(
            "a long enough sentence to pass the floor",
            Author::Human,
            0,
            "suzy",
        );
        assert_eq!(skip_reason(&m, true), Some("addressed to an agent"));
        assert_eq!(skip_reason(&m, false), None);
    }

    #[test]
    fn chatter_is_filtered_before_any_model_is_asked() {
        for short in ["ok", "thanks!", "lol", "👍", "   "] {
            let m = msg(short, Author::Human, 0, "suzy");
            assert!(
                skip_reason(&m, false).is_some(),
                "{short:?} should be skipped"
            );
        }
    }

    #[test]
    fn an_image_with_no_words_is_still_worth_a_look() {
        let mut m = msg("", Author::Human, 0, "suzy");
        m.attachments = vec![crate::control::Attachment {
            hash: "h".into(),
            name: "shot.png".into(),
            mime: "image/png".into(),
            size: 1,
            width: None,
            height: None,
        }];
        assert_eq!(skip_reason(&m, false), None);
    }

    #[test]
    fn an_agent_that_just_spoke_does_not_volunteer_again() {
        let history = vec![msg("I had a thought", Author::Agent, 100, "claude")];
        assert!(spoke_recently(&history, "claude", 110));
        assert!(
            !spoke_recently(&history, "codex", 110),
            "a different agent is free"
        );
        assert!(
            !spoke_recently(&history, "claude", 100 + AMBIENT_QUIET_SECS + 1),
            "the quiet period ends"
        );
    }

    #[test]
    fn pass_is_recognised_however_it_is_typed() {
        for p in ["PASS", "pass", " Pass ", "pAsS\n"] {
            assert!(is_pass(p), "{p:?} is a pass");
        }
    }

    #[test]
    fn a_real_reply_is_not_a_pass() {
        for r in [
            "PASS on that idea",
            "I'll pass this to codex",
            "passing tests are green",
            "",
        ] {
            assert!(!is_pass(r), "{r:?} is not a bare pass");
        }
    }
}
