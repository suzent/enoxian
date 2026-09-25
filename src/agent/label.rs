//! Naming an agent in text that someone — a person or another agent — will read.
//!
//! `ChatMessage::agent_id` and every agent name in config are bare labels, and
//! bare labels collide: one Circle routinely holds two `claude`s on two
//! machines. Rendered bare, a prompt's history reads "claude: …" twice with no
//! way to tell which one is the reader itself, a hold turn can tell claude that
//! "claude answered", and one device's failure notice suppresses another's
//! because they begin with the same words.
//!
//! So an agent is always named with the machine it runs on:
//! `claude (on suzy/jessair)`. That form is deliberately **not** the
//! `@suzy/jessair/claude` handle. The text is going in front of something that
//! is about to write a reply, and a live mention in it invites that mention into
//! the answer, which would wake the named agent on the next pass.
//!
//! Anything that puts an agent's name into prose should go through here. Names
//! used as keys — dedup keys, activity ids, the `--agent` filter — stay bare,
//! because there they are identifiers, not descriptions.

use crate::control::{Author, ChatMessage, MemberEntry, MEMBER_LIST_KEY};
use crate::state::AppState;
use std::collections::HashMap;
use yrs::{Any, Map, Out, ReadTxn, Transact};

/// Who owns each device in the Circle, by peer id.
#[derive(Debug, Default, Clone)]
pub struct Roster {
    by_peer: HashMap<String, (String, String)>,
}

impl Roster {
    /// Read the member list once. A busy or empty control doc gives an empty
    /// roster, under which every name falls back to its bare label — degraded,
    /// but never wrong about who someone is.
    pub fn load(state: &AppState) -> Self {
        let mut by_peer = HashMap::new();
        if let Ok(txn) = state.control.try_transact() {
            if let Some(members) = txn.get_map(MEMBER_LIST_KEY) {
                for (_, value) in members.iter(&txn) {
                    if let Out::Any(Any::String(raw)) = value {
                        if let Ok(m) = serde_json::from_str::<MemberEntry>(&raw) {
                            if !m.owner.is_empty() && !m.device_label.is_empty() {
                                by_peer.insert(m.peer_id, (m.owner, m.device_label));
                            }
                        }
                    }
                }
            }
        }
        Self { by_peer }
    }

    #[cfg(test)]
    pub fn with(entries: &[(&str, &str, &str)]) -> Self {
        Self {
            by_peer: entries
                .iter()
                .map(|(peer, owner, device)| {
                    (peer.to_string(), (owner.to_string(), device.to_string()))
                })
                .collect(),
        }
    }

    /// `agent` running on `peer`, named so it cannot be confused with a
    /// namesake elsewhere.
    pub fn agent(&self, agent: &str, peer: &str) -> String {
        match self.by_peer.get(peer) {
            Some((owner, device)) => format!("{agent} (on {owner}/{device})"),
            None => agent.to_string(),
        }
    }

    /// The name to show for whoever posted `message`.
    ///
    /// Only agents are qualified. A person's `agent_id` already carries a
    /// per-device suffix, and a system post has one author.
    pub fn speaker(&self, message: &ChatMessage) -> String {
        match message.author {
            Author::Agent => self.agent(&message.agent_id, &message.peer_id),
            _ => message.agent_id.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(agent: &str, peer: &str, author: Author) -> ChatMessage {
        ChatMessage {
            thread_root: None,
            id: "m".into(),
            agent_id: agent.into(),
            text: "t".into(),
            mentions: vec![],
            ts: 0,
            peer_id: peer.into(),
            attachments: vec![],
            relay: None,
            author,
            reply_to: None,
        }
    }

    #[test]
    fn two_agents_with_one_name_read_as_two_agents() {
        let roster = Roster::with(&[("A", "suzy", "macbook-pro"), ("B", "suzy", "jessair")]);
        let here = roster.speaker(&said("claude", "A", Author::Agent));
        let there = roster.speaker(&said("claude", "B", Author::Agent));
        assert_eq!(here, "claude (on suzy/macbook-pro)");
        assert_eq!(there, "claude (on suzy/jessair)");
        assert_ne!(here, there);
    }

    #[test]
    fn a_qualified_name_is_never_a_live_mention() {
        // It is written in front of an agent that is about to reply, and a
        // mention copied into that reply would wake the named agent.
        let roster = Roster::with(&[("A", "suzy", "macbook-pro")]);
        assert!(!roster.agent("claude", "A").contains('@'));
    }

    #[test]
    fn a_person_keeps_their_own_label() {
        let roster = Roster::with(&[("A", "suzy", "macbook-pro")]);
        assert_eq!(
            roster.speaker(&said("suzy-ge7Zgn3y", "A", Author::Human)),
            "suzy-ge7Zgn3y"
        );
    }

    #[test]
    fn an_unknown_device_falls_back_to_the_bare_label() {
        // Degraded rather than wrong: better "claude" than a guessed machine.
        assert_eq!(Roster::default().agent("claude", "nowhere"), "claude");
    }
}
