//! Building the prompt enoxian hands to a mentioned agent.
//!
//! The agent's own *memory* is carried by ACP session resume (agent-owned,
//! restored silently on `session/load`). This module supplies the *world
//! context* — what the agent needs to know about the enoxian environment — and,
//! crucially, frames it so the agent does not conversationally reply to the
//! background instead of doing the task.
//!
//! ## Prompt structure
//!
//! Every prompt ends with a single REQUEST the agent should answer. Anything
//! before it is background, wrapped in an explicit CONTEXT block the agent is
//! told not to reply to. This is what prevents the "greeting soup" (the agent
//! answering the brief and each chat line before doing the work).
//!
//! Fresh session (or a session that was lost — the recovery path):
//!
//! ```text
//! The block between <context> tags below is background about your environment.
//! Do NOT reply to it; use it only to inform your response to the REQUEST.
//! <context>
//! <standing brief: who you are, the circle, proposals, that replies go to chat>
//! <member roster>
//! Recent conversation in this room:
//!   <sender>: <text>
//!   ...
//! </context>
//!
//! REQUEST from <sender> (@mention). Respond only to this:
//! <task>
//! ```
//!
//! Resumed session — the agent already holds the brief and history in its own
//! memory, so the CONTEXT block is omitted entirely; only the REQUEST is sent:
//!
//! ```text
//! REQUEST from <sender> (@mention) in circle "<name>". Respond only to this:
//! <task>
//! ```

use crate::control::{ChatMessage, MemberEntry, CHAT_KEY, MEMBER_LIST_KEY};
use crate::state::AppState;
use yrs::{Any, Array, ArrayRef, Map, Out, Transact};

/// How many recent chat lines to include as conversational context.
const RECENT_CHAT_LINES: usize = 12;

/// Compose the prompt. `resumed` omits the background CONTEXT block (the agent
/// already has it via its restored session). See the module docs for the exact
/// shape. The invariant: the prompt always ends with a single REQUEST the agent
/// is told to answer, and any background is fenced as non-conversational.
pub fn build_prompt(
    state: &AppState,
    agent_id: &str,
    sender: &str,
    task: &str,
    resumed: bool,
) -> String {
    // Gather the environment context from the control doc, then compose. The
    // composition itself is pure (`compose`) so it can be unit-tested without an
    // AppState.
    let brief = (!resumed).then(|| standing_brief(state, agent_id));
    let recent = (!resumed)
        .then(|| recent_chat(state))
        .filter(|s| !s.is_empty());
    compose(
        &state.circle_name,
        sender,
        task,
        brief.as_deref(),
        recent.as_deref(),
    )
}

/// Pure prompt composition. `brief`/`recent` are `Some` only for a fresh session
/// (the background CONTEXT block); a resumed session passes `None` for both and
/// gets a request-only prompt. See the module docs for the shape.
fn compose(
    circle_name: &str,
    sender: &str,
    task: &str,
    brief: Option<&str>,
    recent: Option<&str>,
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
            out.push_str("\nRecent conversation in this room:\n");
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
        format!("Members in this circle: {}.\n", members.join(", "))
    };
    format!(
        "You are \"{agent}\", an agent participating in an enoxian circle named \"{circle}\".\n\
         enoxian is a peer-to-peer workspace shared by the members below. You were woken by an \
         @mention in the circle's chat. You are working directly in the shared workspace at the \
         current directory.\n\
         {roster}\
         Anything you write to files here is captured as a reviewable *proposal* that members can \
         accept, reject, or revert — so make focused, clear changes and explain what you did. \
         Your text reply is posted back into the circle chat, so answer conversationally.\n",
        agent = agent_id,
        circle = state.circle_name,
        roster = roster,
    )
}

/// Member display labels (owner + device) for the roster line.
fn member_labels(state: &AppState) -> Vec<String> {
    let map = state.control.get_or_insert_map(MEMBER_LIST_KEY);
    let txn = state.control.transact();
    let mut labels = Vec::new();
    for (_key, val) in map.iter(&txn) {
        if let Out::Any(Any::String(s)) = val {
            if let Ok(m) = serde_json::from_str::<MemberEntry>(&s) {
                let mut label = m.owner.clone();
                if !m.device_label.is_empty() {
                    label.push_str(&format!(" ({})", m.device_label));
                }
                if !m.agents.is_empty() {
                    label.push_str(&format!(" [agents: {}]", m.agents.join(", ")));
                }
                if !label.is_empty() {
                    labels.push(label);
                }
            }
        }
    }
    labels
}

/// The last few chat messages, oldest-first, as `sender: text` lines.
fn recent_chat(state: &AppState) -> String {
    let arr: ArrayRef = state.control.get_or_insert_array(CHAT_KEY);
    let txn = state.control.transact();
    let all: Vec<ChatMessage> = arr
        .iter(&txn)
        .filter_map(|item| {
            if let Out::Any(Any::String(s)) = item {
                serde_json::from_str::<ChatMessage>(&s).ok()
            } else {
                None
            }
        })
        .collect();
    let start = all.len().saturating_sub(RECENT_CHAT_LINES);
    all[start..]
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
    use super::compose;

    #[test]
    fn fresh_prompt_fences_context_and_ends_with_request() {
        let p = compose(
            "delta",
            "suzy",
            "make a test file",
            Some("You are claude, an agent…\n"),
            Some("  suzy: hi\n  claude: hello"),
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
    fn resumed_prompt_is_request_only() {
        let p = compose("delta", "suzy", "make a test file", None, None);
        // No background block on a resumed session.
        assert!(!p.contains("<context>"));
        assert!(!p.contains("Do NOT reply"));
        // Just the request + task.
        assert!(p.starts_with("REQUEST from suzy (@mention) in circle \"delta\""));
        assert!(p.trim_end().ends_with("make a test file"));
    }

    #[test]
    fn brief_without_recent_chat_still_fences() {
        let p = compose("delta", "suzy", "do it", Some("brief text\n"), None);
        assert!(p.contains("<context>"));
        assert!(p.contains("brief text"));
        assert!(!p.contains("Recent conversation")); // no chat section
        assert!(p.contains("REQUEST from suzy"));
    }
}
