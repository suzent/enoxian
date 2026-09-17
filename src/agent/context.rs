//! Prompt context is separate from the agent's persistent ACP conversation.
//!
//! A turn receives a bounded, chronological page after its delivered-input
//! cursor, plus explicit thread ancestors and a standing Circle brief. The
//! cursor advances through supplied input only, never to an outgoing reply.
//! Nothing is put in front of the agent twice: blocks that reach ahead of the
//! cursor are remembered in the record and subtracted from the next turn, and a
//! resumed session is not shown the agent's own posts, which it already holds —
//! and whatever that assumption held back is restored if the resume then fails.
//! Omitted history is named as omitted and can be retrieved through the paginated
//! chat API. The driver identifies failed ACP resume and adds recovery context.
//! Background stays in <context>; the user's request is the final instruction.

use crate::control::{ChatMessage, MemberEntry, MEMBER_LIST_KEY};
use crate::state::AppState;
use yrs::{Any, Map, Out, ReadTxn, Transact};

/// How many recent chat lines to include as conversational context. Also caps
/// the resumed-session delta, so a long absence cannot blow up the prompt.
const RECENT_CHAT_LINES: usize = 12;

/// Heading for the chat block on a fresh session — plain recent history.
const FRESH_CHAT_HEADING: &str = "Recent conversation in this room:";

/// Heading for the chat block on a resumed session. Worded so the agent reads
/// it as "what you missed" rather than a transcript to re-answer; its own
/// previous reply is in there, which is accurate — it is what the room saw.
const DELTA_CHAT_HEADING: &str = "Posted in this room since your last turn:";

/// Compatibility wrapper for callers that need only the prompt text. Runtime
/// callers use `build_delivery` to retain the precise input cursor as well.
pub fn build_prompt(
    state: &AppState,
    agent_id: &str,
    sender: &str,
    task: &str,
    resume: Option<&super::memory::Record>,
    trigger_id: &str,
) -> String {
    build_delivery(state, agent_id, sender, task, resume, trigger_id).prompt
}

/// The most delivered-but-not-yet-passed ids we carry between turns.
///
/// Only ids *ahead* of the contiguous cursor need remembering — anything at or
/// behind it is already excluded by the cursor — and each turn adds at most the
/// two extra blocks below. The cursor then walks forward over them, so the list
/// stays short on its own; this cap only bounds the pathological case of a very
/// long backlog.
const DELIVERED_MEMO: usize = 200;

pub struct Delivery {
    pub prompt: String,
    pub cursor: Option<String>,
    /// Ids ahead of `cursor` that this prompt has already shown the agent, plus
    /// the ones it was still carrying. Persisted with the cursor so the next
    /// turn does not repeat them.
    pub delivered: Vec<String>,
    /// Ids this prompt left out *only* because a resumed session is assumed to
    /// hold them already. That assumption is not tested until `session/load`
    /// runs, so the driver hands these to [`recovery_context`] when the resume
    /// turns out to have failed. Empty on a fresh session.
    pub withheld: Vec<String>,
}

pub fn build_delivery(
    state: &AppState,
    agent_id: &str,
    sender: &str,
    task: &str,
    resume: Option<&super::memory::Record>,
    trigger_id: &str,
) -> Delivery {
    let all = state.transcript();
    let since = resume.map(|r| r.last_seen_message.as_str());
    let after = since
        .and_then(|id| all.iter().position(|m| m.id == id))
        .map(|i| i + 1);
    let start = after.unwrap_or_else(|| all.len().saturating_sub(RECENT_CHAT_LINES));
    let end = (start + RECENT_CHAT_LINES).min(all.len());
    let cursor = all[start..end].last().map(|m| m.id.clone());

    // Nothing goes in front of the agent twice. Two ways it otherwise would:
    //
    //  - The extra blocks below reach past the contiguous cursor, so the next
    //    turn's page walks back over lines they already showed. `carried` is
    //    the memo of exactly those lines.
    //  - A resumed ACP session still holds everything the agent itself said, so
    //    replaying its own posts into the prompt shows it its own words twice —
    //    once as its turn, once quoted back as "the room". Only on a resume: a
    //    fresh session has no such memory, and the recovery path after a failed
    //    resume rebuilds the full window deliberately.
    let carried: std::collections::HashSet<&str> = resume
        .map(|r| r.delivered.iter().map(String::as_str).collect())
        .unwrap_or_default();
    let resumed = resume.is_some();
    let is_own_post = |m: &ChatMessage| m.agent_id == agent_id && m.peer_id == state.peer_id;
    let fresh = |m: &ChatMessage| {
        // The trigger is the REQUEST; it is never also background.
        m.id != trigger_id && !carried.contains(m.id.as_str()) && !(resumed && is_own_post(m))
    };

    let mut supplied: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut shown: Vec<String> = Vec::new();
    let mut withheld: Vec<String> = Vec::new();
    let mut recent = String::new();
    // Keep a contiguous catch-up cursor, but also show the room as it is now
    // and the original trigger's neighborhood after an offline backlog.
    let mut section = |range: std::ops::Range<usize>, heading: Option<&str>| {
        let mut lines: Vec<ChatMessage> = Vec::new();
        for i in range.filter(|i| supplied.insert(*i)) {
            let message = &all[i];
            if message.id == trigger_id {
                continue;
            }
            // Anything held back rests on the resumed session remembering it.
            // Note it so the recovery path can put it back if it did not.
            match fresh(message) {
                true => lines.push(message.clone()),
                false => withheld.push(message.id.clone()),
            }
        }
        if lines.is_empty() {
            return;
        }
        // `window` keeps only the last few lines, so record what it rendered
        // rather than what was offered to it.
        let rendered = window(&lines, None, trigger_id);
        shown.extend(
            lines
                .iter()
                .rev()
                .take(RECENT_CHAT_LINES)
                .map(|m| m.id.clone()),
        );
        match heading {
            Some(heading) => recent.push_str(&format!("\n{heading}\n{rendered}")),
            None => recent.push_str(&rendered),
        }
    };
    section(start..end, None);
    section(
        all.len().saturating_sub(RECENT_CHAT_LINES)..all.len(),
        Some("Latest room context:"),
    );
    if let Some(trigger) = all.iter().position(|m| m.id == trigger_id) {
        section(
            trigger.saturating_sub(6)..(trigger + 6).min(all.len()),
            Some("Context around the original request:"),
        );
    }
    if end < all.len() || (after.is_none() && start > 0) {
        recent.push_str(&format!("\nHistory omitted from this prompt. Retrieve pages from GET /circles/{}/api/chat?after_id={}&limit=100. Omitted lines have NOT been delivered.", state.circle_id, if after.is_some() { cursor.as_deref().unwrap_or("") } else { "" }));
    }
    // Include ancestors of the explicit request even if outside the room page.
    // `supplied` covers every block above, not just the contiguous page, so an
    // ancestor that the latest-room or trigger-neighborhood block already
    // printed is not printed a second time under its own heading.
    let mut parent = all
        .iter()
        .find(|m| m.id == trigger_id)
        .and_then(|m| m.reply_to.clone());
    let mut visited = std::collections::HashSet::new();
    while let Some(id) = parent {
        if !visited.insert(id.clone()) || visited.len() > 32 {
            break;
        }
        let Some(index) = all.iter().position(|m| m.id == id) else {
            break;
        };
        let message = &all[index];
        if supplied.insert(index) && message.id != trigger_id {
            if fresh(message) {
                shown.push(message.id.clone());
                recent.push_str(&format!(
                    "\nThread ancestor {} ({} on {}): {}",
                    message.id, message.agent_id, message.peer_id, message.text
                ));
            } else {
                withheld.push(message.id.clone());
            }
        }
        parent = message.reply_to.clone();
    }
    // Keep the brief available even when session/load falls back to session/new.
    let brief = standing_brief(state, agent_id);
    Delivery {
        prompt: compose(
            &state.circle_name,
            sender,
            task,
            Some(&brief),
            (!recent.trim().is_empty()).then_some(recent.as_str()),
            if resumed {
                DELTA_CHAT_HEADING
            } else {
                FRESH_CHAT_HEADING
            },
        ),
        delivered: carry_forward(&all, start, carried, shown, trigger_id),
        withheld,
        cursor,
    }
}

/// The memo to store with the new cursor.
///
/// Drops everything the cursor now covers (`start` is the first line the next
/// turn will page in, so anything before it can never come back) and everything
/// that has left the transcript, then keeps the most recent ids within the cap.
fn carry_forward(
    all: &[ChatMessage],
    start: usize,
    carried: std::collections::HashSet<&str>,
    shown: Vec<String>,
    trigger_id: &str,
) -> Vec<String> {
    let ahead: std::collections::HashSet<&str> = all[start.min(all.len())..]
        .iter()
        .map(|m| m.id.as_str())
        .collect();
    let mut memo: Vec<String> = carried
        .into_iter()
        .filter(|id| ahead.contains(id))
        .map(str::to_string)
        .collect();
    memo.extend(
        shown
            .into_iter()
            .chain(std::iter::once(trigger_id.to_string()))
            .filter(|id| ahead.contains(id.as_str())),
    );
    memo.sort_unstable();
    memo.dedup();
    if memo.len() > DELIVERED_MEMO {
        // Keep the newest: transcript order, not the sort order above.
        let keep: std::collections::HashSet<&str> = memo.iter().map(String::as_str).collect();
        memo = all
            .iter()
            .rev()
            .filter(|m| keep.contains(m.id.as_str()))
            .take(DELIVERED_MEMO)
            .map(|m| m.id.clone())
            .collect();
    }
    memo
}

/// Context prepended when `session/load` failed and the agent starts fresh.
///
/// `withheld` is what the prompt left out on the assumption that the resumed
/// session still held it (see [`Delivery::withheld`]). That assumption has just
/// proved wrong, so those lines are restored here — otherwise they would be
/// skipped for good, the cursor having advanced past them. Lines the room
/// history below already shows are not repeated.
pub fn recovery_context(state: &AppState, agent: &str, withheld: &[String]) -> String {
    let all = state.transcript();
    let history = window(&all, None, "");
    // The history block below is the room's last few lines; anything inside it
    // is already on screen.
    let tail: std::collections::HashSet<&str> = all[all.len().saturating_sub(RECENT_CHAT_LINES)..]
        .iter()
        .map(|m| m.id.as_str())
        .collect();
    let restored: Vec<&ChatMessage> = withheld
        .iter()
        .filter(|id| !tail.contains(id.as_str()))
        .filter_map(|id| all.iter().find(|m| &m.id == id))
        .collect();
    let restored = if restored.is_empty() {
        String::new()
    } else {
        format!(
            "Earlier lines you were assumed to remember:\n{}\n",
            restored
                .iter()
                .map(|m| format!("  {}: {}", m.agent_id, m.text.replace('\n', " ")))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    format!("<context>\nThe previous ACP conversation could not be restored. Its private memory is unavailable.\n{}\n{restored}Recent room history:\n{history}\nFor older history, retrieve GET /circles/{}/api/chat?limit=100, then continue with after_id equal to the last returned message ID.\n</context>\n\n",
        standing_brief(state, agent), state.circle_id)
}

/// Pure prompt composition. `brief` is `Some` only for a fresh session; `recent`
/// is the chat block (full history when fresh, the delta when resumed) and is
/// `None` when there is nothing to show. Either one present produces the fenced
/// background CONTEXT block. See the module docs for the shape.
fn compose(
    circle_name: &str,
    sender: &str,
    task: &str,
    brief: Option<&str>,
    recent: Option<&str>,
    recent_heading: &str,
) -> String {
    let mut out = String::new();

    if brief.is_some() || recent.is_some() {
        // Background, fenced and explicitly marked "do not reply to this" so the
        // agent does not answer the brief/chat conversationally before the task.
        out.push_str(
            "The block between <context> tags below is background about your \
             environment. Do NOT reply to it; use it only to inform your response \
             to the REQUEST that follows.\n<context>\n",
        );
        if let Some(brief) = brief {
            out.push_str(brief);
        }
        if let Some(recent) = recent {
            out.push('\n');
            out.push_str(recent_heading);
            out.push('\n');
            out.push_str(recent);
            out.push('\n');
        }
        out.push_str("</context>\n\n");
    }

    // The single REQUEST the agent should answer — always last, always the only
    // thing framed as something to respond to.
    out.push_str(&format!(
        "REQUEST from {sender} (@mention) in circle \"{circle_name}\". Respond only to this:\n"
    ));
    out.push_str(task);
    out
}

/// The standing brief describing the enoxian environment. Sent once per fresh
/// session; the agent carries it forward via resume after that.
fn standing_brief(state: &AppState, agent_id: &str) -> String {
    let members = member_labels(state);
    let roster = if members.is_empty() {
        String::new()
    } else {
        format!("Members in this circle: {}.\n", members.join("; "))
    };
    // Which machine this agent is on, and how to address it exactly.
    //
    // Without this an agent knows its own *name* but not which device it runs
    // on, and the roster shows several devices that may each configure an agent
    // by that same name. It cannot then tell itself apart from its namesakes,
    // answer "which one are you?", or address a specific one — the reason a
    // user ends up saying "mention the one on <device>" by hand.
    // The grammar holds whether or not this device knows its own name, so it is
    // stated unconditionally; only the "you are here" half is conditional. An
    // agent that cannot place itself can still address someone else correctly,
    // and the roster above now spells the handles out.
    let addressing = match self_identity(state) {
        Some((owner, device)) => format!(
            "You are running on device \"{device}\" (owner \"{owner}\"). Your own address is \
             @{owner}/{device}/{agent}.\n\
             To address someone, copy their handle from the member list below: @owner/device/agent \
             runs that device's agent, while @owner or @owner/device only notifies a person or a \
             machine. A bare @{agent} is not the same as your handle — it may reach a different \
             device's agent of the same name.\n",
            agent = agent_id,
        ),
        None => format!(
            "To address someone, copy their handle from the member list below: @owner/device/agent \
             runs that device's agent, while @owner or @owner/device only notifies a person or a \
             machine. A bare @{agent} may reach a different device's agent of the same name.\n",
            agent = agent_id,
        ),
    };
    format!(
        "You are \"{agent}\", an agent participating in an enoxian circle named \"{circle}\".\n\
         enoxian is a peer-to-peer workspace shared by the members below. You were woken by an \
         @mention in the circle's chat. You are working directly in the shared workspace at the \
         current directory.\n\
         {addressing}\
         {roster}\
         Anything you write to files here is captured as a reviewable *proposal* that members can \
         accept, reject, or revert — so make focused, clear changes and explain what you did. \
         Your text reply is posted back into the circle chat, so answer conversationally.\n\
         If another agent is clearly better placed for part of the work, you may hand it over by \
         mentioning it. Whether it actually runs is that device's own decision, only the first \
         agent you mention is woken, and a chain of hand-offs shares a limited budget — so do the \
         work yourself unless delegating is plainly better.\n",
        agent = agent_id,
        circle = state.circle_name,
        addressing = addressing,
        roster = roster,
    )
}

/// This device's own `(owner, device_label)` as the Circle knows it.
fn self_identity(state: &AppState) -> Option<(String, String)> {
    let me = state.self_member()?;
    (!me.owner.is_empty() && !me.device_label.is_empty()).then_some((me.owner, me.device_label))
}

/// Member display labels (owner + device) for the roster line.
fn member_labels(state: &AppState) -> Vec<String> {
    let Ok(txn) = state.control.try_transact() else {
        return Vec::new();
    };
    let Some(map) = txn.get_map(MEMBER_LIST_KEY) else {
        return Vec::new();
    };
    let mut labels = Vec::new();
    for (_key, val) in map.iter(&txn) {
        if let Out::Any(Any::String(s)) = val {
            if let Ok(m) = serde_json::from_str::<MemberEntry>(&s) {
                let mut label = m.owner.clone();
                if !m.device_label.is_empty() {
                    label.push_str(&format!(" on {}", m.device_label));
                }
                if m.peer_id == state.peer_id {
                    label.push_str(" (this device)");
                }
                // Spell out the handle rather than the display label. The
                // roster used to read `suzy (jessair) [agents: claude]`, which
                // left an agent to work out `@suzy/jessair/claude` from parens
                // and brackets — a guess it does not need to make, and one it
                // cannot check. These are the exact strings a mention takes.
                if !m.agents.is_empty() {
                    let handles: Vec<String> = m
                        .agents
                        .iter()
                        .map(|agent| {
                            if m.owner.is_empty() || m.device_label.is_empty() {
                                format!("@{agent}")
                            } else {
                                format!("@{}/{}/{}", m.owner, m.device_label, agent)
                            }
                        })
                        .collect();
                    label.push_str(&format!(" — {}", handles.join(", ")));
                }
                if !label.is_empty() {
                    labels.push(label);
                }
            }
        }
    }
    labels
}

/// Chat lines as `sender: text`, oldest-first, capped at [`RECENT_CHAT_LINES`].
///
/// `since` bounds the window to messages posted after that message id — the
/// resumed-session delta. `Some("")` (a record predating the seen-mark) and a
/// mark no longer in the log both fall back to the last few lines, which is
/// bounded and more useful than sending nothing. `exclude` drops the mention
/// being handled, since it is already the REQUEST.
/// The chat window, split out from the yrs read so it can be unit-tested.
fn window(all: &[ChatMessage], since: Option<&str>, exclude: &str) -> String {
    let after = since
        .filter(|mark| !mark.is_empty())
        .and_then(|mark| all.iter().position(|m| m.id == mark))
        .map(|idx| idx + 1);
    let lines: Vec<&ChatMessage> = all[after.unwrap_or(0)..]
        .iter()
        .filter(|m| m.id != exclude)
        .collect();
    let start = lines.len().saturating_sub(RECENT_CHAT_LINES);
    lines[start..]
        .iter()
        .map(|m| {
            let text = m.text.replace('\n', " ");
            format!("  {}: {}", m.agent_id, text)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: &str, sender: &str, text: &str) -> ChatMessage {
        ChatMessage {
            thread_root: None,
            id: id.to_string(),
            agent_id: sender.to_string(),
            text: text.to_string(),
            mentions: Vec::new(),
            ts: 0,
            peer_id: String::new(),
            attachments: Vec::new(),
            relay: None,
            author: crate::control::Author::Human,
            reply_to: None,
        }
    }

    #[test]
    fn fresh_prompt_fences_context_and_ends_with_request() {
        let p = compose(
            "delta",
            "suzy",
            "make a test file",
            Some("You are claude, an agent…\n"),
            Some("  suzy: hi\n  claude: hello"),
            FRESH_CHAT_HEADING,
        );
        // Background is fenced and flagged do-not-reply.
        assert!(p.contains("Do NOT reply to it"));
        assert!(p.contains("<context>") && p.contains("</context>"));
        assert!(p.contains("Recent conversation in this room:"));
        // The request is present, labelled, and LAST.
        assert!(p.contains("REQUEST from suzy"));
        assert!(p.trim_end().ends_with("make a test file"));
        // The context block comes before the request.
        assert!(p.find("<context>").unwrap() < p.find("REQUEST from").unwrap());
    }

    #[test]
    fn resumed_prompt_with_no_new_chat_is_request_only() {
        let p = compose(
            "delta",
            "suzy",
            "make a test file",
            None,
            None,
            DELTA_CHAT_HEADING,
        );
        // No background block when nothing happened since the last turn.
        assert!(!p.contains("<context>"));
        assert!(!p.contains("Do NOT reply"));
        // Just the request + task.
        assert!(p.starts_with("REQUEST from suzy (@mention) in circle \"delta\""));
        assert!(p.trim_end().ends_with("make a test file"));
    }

    #[test]
    fn resumed_prompt_fences_the_delta_without_the_brief() {
        let p = compose(
            "delta",
            "suzy",
            "ok go ahead",
            None,
            Some("  bob: actually use the backoff helper"),
            DELTA_CHAT_HEADING,
        );
        // The delta is fenced and flagged do-not-reply, like any background.
        assert!(p.contains("<context>") && p.contains("Do NOT reply to it"));
        assert!(p.contains("Posted in this room since your last turn:"));
        assert!(p.contains("bob: actually use the backoff helper"));
        // But the standing brief is not repeated.
        assert!(!p.contains("an agent participating in an enoxian circle"));
        assert!(p.trim_end().ends_with("ok go ahead"));
    }

    #[test]
    fn brief_without_recent_chat_still_fences() {
        let p = compose(
            "delta",
            "suzy",
            "do it",
            Some("brief text\n"),
            None,
            FRESH_CHAT_HEADING,
        );
        assert!(p.contains("<context>"));
        assert!(p.contains("brief text"));
        assert!(!p.contains("Recent conversation")); // no chat section
        assert!(p.contains("REQUEST from suzy"));
    }

    #[test]
    fn delta_starts_after_the_seen_mark() {
        let all = [
            msg("m1", "suzy", "@claude add a retry"),
            msg("m2", "claude", "done — added a retry"),
            msg("m3", "bob", "actually we decided against retries"),
            msg("m4", "suzy", "@claude ok go ahead"),
        ];
        // Seen through its own reply (m2); the trigger (m4) is the REQUEST.
        let out = window(&all, Some("m2"), "m4");
        assert_eq!(out, "  bob: actually we decided against retries");
    }

    #[test]
    fn delta_is_empty_when_nothing_happened_in_between() {
        let all = [
            msg("m1", "suzy", "@claude hi"),
            msg("m2", "claude", "hello"),
            msg("m3", "suzy", "@claude again"),
        ];
        assert!(window(&all, Some("m2"), "m3").is_empty());
    }

    #[test]
    fn fresh_window_is_the_last_lines_without_the_trigger() {
        let all = [
            msg("m1", "suzy", "hi"),
            msg("m2", "bob", "hey"),
            msg("m3", "suzy", "@claude do it"),
        ];
        let out = window(&all, None, "m3");
        assert_eq!(out, "  suzy: hi\n  bob: hey");
    }

    #[test]
    fn unknown_or_empty_mark_falls_back_to_recent_lines() {
        let all = [msg("m1", "suzy", "hi"), msg("m2", "suzy", "@claude do it")];
        // A record predating the seen-mark, and a mark trimmed from the log.
        assert_eq!(window(&all, Some(""), "m2"), "  suzy: hi");
        assert_eq!(window(&all, Some("gone"), "m2"), "  suzy: hi");
    }

    #[test]
    fn window_is_capped_even_after_a_long_absence() {
        let all: Vec<ChatMessage> = (0..40)
            .map(|i| msg(&format!("m{i}"), "suzy", &format!("line {i}")))
            .collect();
        let out = window(&all, Some("m0"), "m39");
        assert_eq!(out.lines().count(), RECENT_CHAT_LINES);
        // Capped to the *most recent* lines, ending just before the trigger.
        assert!(out.ends_with("  suzy: line 38"));
    }

    #[test]
    fn newlines_in_a_message_are_flattened_to_one_line() {
        let all = [msg("m1", "suzy", "first\nsecond")];
        assert_eq!(window(&all, None, ""), "  suzy: first second");
    }
}
