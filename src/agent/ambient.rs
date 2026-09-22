//! Ambient engagement: letting an agent read the room and volunteer.
//!
//! See `docs/guide/agents.md`, "Agents that read the room". An agent configured
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
///
/// It no longer opens with "You were not addressed". That sentence existed to
/// undo the `REQUEST from <sender> (@mention)` header the prompt had already
/// asserted above it; now that an unaddressed turn is framed as overheard from
/// the start, repeating the denial here would be the contradiction in the other
/// direction. See `docs/concepts/internals.md`, "Agent Runtime".
///
/// `co_listeners` is every agent shown this message, `self_id` included. What it
/// buys is a reason to pass that is not self-deprecation: an agent with nothing
/// uniquely useful to say can leave it to someone who has, rather than weighing
/// its own contribution in a vacuum.
pub fn ambient_instruction(co_listeners: &[String], self_id: &str) -> String {
    let others: Vec<&str> = co_listeners
        .iter()
        .map(String::as_str)
        .filter(|name| !name.eq_ignore_ascii_case(self_id))
        .collect();
    let shared = if others.is_empty() {
        String::new()
    } else {
        format!(
            " {} {} also deciding whether to answer this, so you do not have to cover everything \
             — pass if someone else is better placed.",
            others.join(" and "),
            if others.len() == 1 { "is" } else { "are" },
        )
    };
    format!(
        "If you have nothing worth adding, reply with exactly PASS and nothing else. Do not \
         explain why you are passing. Passing is the common case and is not a failure: most of \
         what is said in a room does not need an agent.{shared} Someone may also have answered \
         while this turn was waiting to run — check the recent history above, and pass if the \
         point has been made. This is a conversational turn, not a work order: do not change \
         files. If something needs doing, say so and let someone ask for it."
    )
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
    fn the_instruction_no_longer_argues_with_its_own_prompt() {
        // "You were not addressed" existed only to undo a header that now says
        // the right thing. Keeping both would contradict in the other direction.
        let solo = ["claude".to_string()];
        let text = ambient_instruction(&solo, "claude");
        assert!(!text.contains("You were not addressed"));
        assert!(text.contains("reply with exactly PASS"));
        assert!(text.contains("do not change files"));
        assert!(
            !text.contains("also deciding"),
            "a sole listener has no company to defer to"
        );
    }

    #[test]
    fn a_listener_is_told_who_else_could_answer() {
        let pair = ["claude".to_string(), "codex".to_string()];
        let text = ambient_instruction(&pair, "claude");
        assert!(text.contains("codex is also deciding whether to answer this"));
        assert!(!text.contains("claude is also"), "never about itself");

        let trio = ["claude".to_string(), "codex".to_string(), "pi".to_string()];
        let text = ambient_instruction(&trio, "CLAUDE");
        assert!(
            text.contains("codex and pi are also deciding"),
            "self-match ignores case, and the verb agrees"
        );
    }

    #[test]
    fn every_listener_is_told_the_point_may_already_be_made() {
        // A turn can wait behind a device permit while someone else answers.
        // The queue cancels what it can see; this covers the rest of the race.
        let text = ambient_instruction(&["claude".to_string()], "claude");
        assert!(text.contains("Someone may also have answered"));
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
