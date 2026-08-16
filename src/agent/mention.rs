//! Structured chat mention parsing.
//!
//! A mention addresses one of three levels in the member hierarchy:
//!
//! ```text
//! @owner                     -> a whole user (all their devices)   [notify]
//! @owner/device              -> one device                          [notify]
//! @owner/device/agent        -> one device's agent                  [run]
//! @agent                     -> bare agent name (any device that    [run]
//!                               allowlists it) — legacy form
//! ```
//!
//! Only agent-level targets launch anything; user/device targets are notify
//! wake signals for now. Bare `@agent` keeps the original behavior so existing
//! usage and the single-word case still work.
//!
//! The raw mention string stored on a `ChatMessage` is the slash-joined form
//! (e.g. `alice/laptop/claude`); [`Mention::parse`] turns it back into
//! structure. The reaction loop uses [`Mention::agent_target`] to decide
//! whether — and where — to run.

/// A parsed mention target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mention {
    /// `@owner` — the whole user.
    User { owner: String },
    /// `@owner/device` — a specific device.
    Device { owner: String, device: String },
    /// `@owner/device/agent` — a specific device's agent.
    Agent {
        owner: String,
        device: String,
        agent: String,
    },
    /// `@agent` — bare agent name, not scoped to any device (legacy). Any
    /// device that allowlists `agent` may react.
    BareAgent { agent: String },
}

impl Mention {
    /// Parse a stored mention string (the slash-joined body without the `@`).
    /// Empty or all-empty-segment input yields `None`.
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim().trim_start_matches('@');
        let parts: Vec<&str> = raw.split('/').filter(|s| !s.is_empty()).collect();
        match parts.as_slice() {
            [agent] => Some(Mention::BareAgent {
                agent: (*agent).to_string(),
            }),
            [owner, device] => Some(Mention::Device {
                owner: (*owner).to_string(),
                device: (*device).to_string(),
            }),
            [owner, device, agent] => Some(Mention::Agent {
                owner: (*owner).to_string(),
                device: (*device).to_string(),
                agent: (*agent).to_string(),
            }),
            // More than three segments: treat the first two as owner/device and
            // the remainder joined as the agent (agent names shouldn't contain
            // '/', but be lenient rather than drop the mention).
            [owner, device, rest @ ..] if !rest.is_empty() => Some(Mention::Agent {
                owner: (*owner).to_string(),
                device: (*device).to_string(),
                agent: rest.join("/"),
            }),
            _ => None,
        }
    }

    /// If this mention names an agent to run, return `(agent_name, scope)`.
    /// `scope` is `Some((owner, device))` for a device-qualified target, or
    /// `None` for a bare agent that any allowlisting device may run.
    /// User- and device-level mentions return `None` (notify-only).
    pub fn agent_target(&self) -> Option<(&str, Option<(&str, &str)>)> {
        match self {
            Mention::Agent {
                owner,
                device,
                agent,
            } => Some((agent, Some((owner, device)))),
            Mention::BareAgent { agent } => Some((agent, None)),
            Mention::User { .. } | Mention::Device { .. } => None,
        }
    }
}

/// Extract all mention strings from chat text, in the slash-joined body form
/// (no leading `@`). Recognizes `owner/device/agent` scoping; a segment may
/// contain letters, digits, `-`, and `_`. Trailing punctuation ends a mention.
pub fn extract(text: &str) -> Vec<String> {
    let mut mentions = Vec::new();
    for word in text.split_whitespace() {
        let Some(rest) = word.strip_prefix('@') else {
            continue;
        };
        let body: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_' || *c == '/')
            .collect();
        // Normalize: drop trailing slash, collapse empty segments.
        let normalized: Vec<&str> = body.split('/').filter(|s| !s.is_empty()).collect();
        if !normalized.is_empty() {
            mentions.push(normalized.join("/"));
        }
    }
    mentions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_all_levels() {
        let text = "@alice ping @alice/laptop and @alice/laptop/claude please";
        assert_eq!(
            extract(text),
            vec!["alice", "alice/laptop", "alice/laptop/claude"]
        );
    }

    #[test]
    fn trailing_punctuation_ends_mention() {
        assert_eq!(
            extract("@alice/laptop/claude, go"),
            vec!["alice/laptop/claude"]
        );
        assert_eq!(extract("hi @bob!"), vec!["bob"]);
    }

    #[test]
    fn ignores_email_and_bare_at() {
        assert_eq!(extract("mail me at foo@bar.com"), Vec::<String>::new());
        assert_eq!(extract("@ not a mention"), Vec::<String>::new());
    }

    #[test]
    fn parse_levels() {
        assert_eq!(
            Mention::parse("claude"),
            Some(Mention::BareAgent {
                agent: "claude".into()
            })
        );
        assert_eq!(
            Mention::parse("alice/laptop"),
            Some(Mention::Device {
                owner: "alice".into(),
                device: "laptop".into()
            })
        );
        assert_eq!(
            Mention::parse("alice/laptop/claude"),
            Some(Mention::Agent {
                owner: "alice".into(),
                device: "laptop".into(),
                agent: "claude".into(),
            })
        );
        assert_eq!(Mention::parse(""), None);
    }

    #[test]
    fn agent_target_only_for_agents() {
        assert_eq!(
            Mention::parse("alice/laptop/claude")
                .unwrap()
                .agent_target(),
            Some(("claude", Some(("alice", "laptop"))))
        );
        assert_eq!(
            Mention::parse("claude").unwrap().agent_target(),
            Some(("claude", None))
        );
        // User / device mentions do not launch.
        assert!(
            Mention::parse("alice").unwrap().agent_target().is_some(),
            "bare single token is treated as an agent (legacy)"
        );
        assert_eq!(Mention::parse("alice/laptop").unwrap().agent_target(), None);
    }
}
