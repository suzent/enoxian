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
///
/// Measured in [`weighted_len`] units, not characters — see why there.
const MIN_AMBIENT_CHARS: usize = 24;

/// What a Han ideograph or Hangul syllable is worth against the threshold.
///
/// [`MIN_AMBIENT_CHARS`] was calibrated on English, where a word runs about
/// four characters plus its space. A Han character is a whole morpheme, so
/// counting it as one made the gate scale with the writing system rather than
/// the content: 今天天气怎样 — "how's the weather today", 23 characters in
/// English — counts 6 and never cleared the bar. The effect was that ambient
/// agents were silent in a Chinese-language room while addressed mentions kept
/// working, which reads exactly like a broken feature.
const MORPHEME_WEIGHT: usize = 4;

/// What a kana is worth. Kana are syllables rather than morphemes, and
/// Japanese interleaves them with kanji that carry the weight already.
const SYLLABLE_WEIGHT: usize = 2;

fn char_weight(c: char) -> usize {
    match c as u32 {
        // Han: CJK Unified Ideographs, Extension A, and compatibility forms.
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF => MORPHEME_WEIGHT,
        // Han beyond the BMP (Extensions B onward).
        0x20000..=0x3FFFF => MORPHEME_WEIGHT,
        // Hangul syllables.
        0xAC00..=0xD7AF => MORPHEME_WEIGHT,
        // Hiragana and katakana.
        0x3040..=0x30FF => SYLLABLE_WEIGHT,
        _ => 1,
    }
}

/// Length of `text` in units comparable to English characters.
///
/// Only the threshold comparison uses this; nothing else about a message is
/// weighted.
fn weighted_len(text: &str) -> usize {
    text.chars().map(char_weight).sum()
}

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
    if weighted_len(msg.text.trim()) < MIN_AMBIENT_CHARS && msg.attachments.is_empty() {
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
    fn a_chinese_question_is_not_mistaken_for_chatter() {
        // Regression: these are real questions a human asked in a Circle and
        // got no ambient answer to, because the floor counted characters.
        for question in [
            "今天天气怎样",           // "how's the weather today"
            "没有人回答我吗",         // "is no one going to answer me"
            "明天天气怎样？？？？？", // with trailing punctuation
        ] {
            let m = msg(question, Author::Human, 0, "suzy");
            assert_eq!(
                skip_reason(&m, false),
                None,
                "{question:?} should be offered an ambient turn"
            );
        }
    }

    #[test]
    fn short_cjk_chatter_is_still_filtered() {
        // The floor still has to do its job: a greeting is not a question.
        for chatter in ["好的", "谢谢", "有人在吗", "はい"] {
            let m = msg(chatter, Author::Human, 0, "suzy");
            assert!(
                skip_reason(&m, false).is_some(),
                "{chatter:?} should not spend a model turn"
            );
        }
    }

    #[test]
    fn weighting_leaves_latin_text_measured_by_characters() {
        // English behaviour must be unchanged: the weights only apply to CJK.
        assert_eq!(weighted_len("but how to setup the env"), 24);
        assert_eq!(weighted_len("anyone?"), 7);
        assert_eq!(weighted_len("thanks 👍"), 8);
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
