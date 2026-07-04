//! Building the prompt enoxian hands to a mentioned agent.
//!
//! Two kinds of context (see the design discussion): the agent's own *memory*
//! is carried by ACP session resume; this module supplies the *world context* —
//! what the agent needs to know about the enoxian environment it is acting in.
//!
//! On a fresh session we prepend a standing brief (what enoxian is, that file
//! changes become reviewable proposals, who is in the room). On a resumed
//! session the agent already has that history, so we send only a lean per-turn
//! header (who mentioned it now) plus the task.

use crate::control::{ChatMessage, MemberEntry, CHAT_KEY, MEMBER_LIST_KEY};
use crate::state::AppState;
use yrs::{Any, Array, ArrayRef, Map, Out, Transact};

/// How many recent chat lines to include as conversational context.
const RECENT_CHAT_LINES: usize = 12;

/// Compose the full prompt: world context (brief and/or per-turn header) + the
/// user's task. `resumed` selects the lean header vs. the full standing brief.
pub fn build_prompt(
    state: &AppState,
    agent_id: &str,
    sender: &str,
    task: &str,
    resumed: bool,
) -> String {
    let mut out = String::new();

    if resumed {
        // Agent already has the standing brief in its resumed history.
        out.push_str(&format!(
            "[enoxian] {sender} mentioned you (@{agent_id}) in circle \"{}\".\n\n",
            state.circle_name
        ));
    } else {
        out.push_str(&standing_brief(state, agent_id));
        let recent = recent_chat(state);
        if !recent.is_empty() {
            out.push_str("\nRecent conversation in this room:\n");
            out.push_str(&recent);
            out.push('\n');
        }
        out.push_str(&format!("\n{sender} mentioned you with this request:\n"));
    }

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
