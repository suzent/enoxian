//! Agent cards: what each agent in a Circle says it is good for.
//!
//! A card is a Markdown file in the shared folder, one per agent per device, at
//! `agents/<owner>/<device>/<agent>.md` — the path spells the agent's handle, so
//! the card for `@suzy/jessair/claude` is `agents/suzy/jessair/claude.md`. Per
//! device rather than per name, because two `claude`s on two machines can run
//! different models with different tools.
//!
//! Cards are ordinary shared files on purpose. They sync like any other file,
//! and an edit — by a member or by the agent itself — is a proposal someone
//! reviews and can revert, so a card needs no permissions of its own.
//!
//! What a card holds is a description, never a statistic. Cost and success
//! rates stay on the device that paid for them (see `usage`); writing them here
//! would publish them to every member and turn each update into a proposal. The
//! warden reads a card as the description of an option, and corrects for how
//! wrong a self-description can be from outcomes, not from the card.

use crate::control::{MemberEntry, MEMBER_LIST_KEY};
use crate::state::AppState;
use std::path::{Path, PathBuf};
use yrs::{Any, Map, Out, ReadTxn, Transact};

/// The folder cards live under, at the root of the shared folder.
pub const DIR: &str = "agents";

/// How much of an agent's own card goes into its brief.
const OWN_CARD_LIMIT: usize = 1_500;

/// How long one line about another agent may be.
pub const SUMMARY_LIMIT: usize = 160;

/// How many other agents the brief lists. A Circle with more than this many
/// agents is not one where an agent picks who to hand over to from a list.
const OTHERS_LIMIT: usize = 16;

/// One agent's card, as read from the shared folder.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Card {
    pub owner: String,
    pub device: String,
    pub agent: String,
    /// Relative to the shared folder, with `/` separators.
    pub path: String,
    /// One line for other agents and for the warden. Empty when the card does
    /// not say anything yet.
    pub summary: String,
    pub body: String,
}

impl Card {
    pub fn handle(&self) -> String {
        format!("@{}/{}/{}", self.owner, self.device, self.agent)
    }
}

/// A path segment that cannot climb out of the folder or nest further.
///
/// Owners and device labels are typed by people. A `/` would split one segment
/// into two, and a leading `.` would hide the file or point at a parent.
fn segment(text: &str) -> String {
    let cleaned: String = text
        .trim()
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim_start_matches('.');
    if cleaned.is_empty() {
        "_".to_string()
    } else {
        cleaned.to_string()
    }
}

/// Where the card for `@owner/device/agent` lives, relative to the shared folder.
pub fn relative_path(owner: &str, device: &str, agent: &str) -> String {
    format!(
        "{DIR}/{}/{}/{}.md",
        segment(owner),
        segment(device),
        segment(agent)
    )
}

fn absolute(workspace: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(workspace.to_path_buf(), |path, part| path.join(part))
}

/// Read one card, or `None` if it does not exist.
pub fn read(workspace: &Path, owner: &str, device: &str, agent: &str) -> Option<Card> {
    let path = relative_path(owner, device, agent);
    let body = std::fs::read_to_string(absolute(workspace, &path)).ok()?;
    Some(Card {
        owner: owner.to_string(),
        device: device.to_string(),
        agent: agent.to_string(),
        summary: summary_of(&body),
        path,
        body,
    })
}

/// The one line a card offers about itself.
///
/// The `summary:` field of its front matter when that says something, else the
/// first line of prose. Headings and HTML comments are skipped: the template's
/// guidance is in comments, and an untouched template must read as empty rather
/// than as an agent describing itself with the instructions for doing so.
pub fn summary_of(text: &str) -> String {
    let mut lines = text.lines().peekable();
    let mut front_summary = None;
    if lines.peek().is_some_and(|l| l.trim() == "---") {
        lines.next();
        for line in lines.by_ref() {
            let line = line.trim();
            if line == "---" {
                break;
            }
            if let Some(value) = line.strip_prefix("summary:") {
                let value = value.trim().trim_matches(|c| c == '"' || c == '\'').trim();
                if !value.is_empty() {
                    front_summary = Some(value.to_string());
                }
            }
        }
    }
    let line = front_summary.or_else(|| {
        let mut in_comment = false;
        for line in lines {
            let mut line = line.trim();
            if in_comment {
                match line.find("-->") {
                    Some(end) => {
                        in_comment = false;
                        line = line[end + 3..].trim();
                    }
                    None => continue,
                }
            }
            if let Some(start) = line.find("<!--") {
                match line[start..].find("-->") {
                    Some(_) => {}
                    None => in_comment = true,
                }
                line = line[..start].trim();
            }
            let line = line.trim_start_matches(['-', '*', ' ']).trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            return Some(line.to_string());
        }
        None
    });
    line.map(|l| clip(&l.replace(['\r', '\n'], " "), SUMMARY_LIMIT))
        .unwrap_or_default()
}

fn clip(text: &str, limit: usize) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        format!("{}…", head.trim_end())
    } else {
        head
    }
}

/// A card nobody has written yet. Everything to fill in is a comment, so an
/// untouched template summarizes to nothing.
pub fn template(owner: &str, device: &str, agent: &str) -> String {
    format!(
        "---\n\
         summary: \"\"\n\
         ---\n\
         # @{owner}/{device}/{agent}\n\
         \n\
         <!-- One line in `summary` above is what other agents and the warden see first.\n\
         Describe what this agent is, not how well it has done: outcomes are measured, not\n\
         written here. Edits are reviewed like any other change. -->\n\
         \n\
         ## Model\n\
         <!-- Which model or product runs behind this agent. -->\n\
         \n\
         ## Can use\n\
         <!-- Tools it has here: shell, web, files, tests, a particular checkout. -->\n\
         \n\
         ## Good for\n\
         <!-- The questions and work to send its way. -->\n\
         \n\
         ## Not for\n\
         <!-- What to ask someone else instead. -->\n"
    )
}

/// Write the template for `@owner/device/agent` unless a card is already there.
///
/// Returns the relative path, and whether this call created it. Disk only: the
/// API creates cards through the file API instead, which also seeds the shared
/// document (see `api::agent_cards`).
#[cfg(test)]
pub fn create(
    workspace: &Path,
    owner: &str,
    device: &str,
    agent: &str,
) -> std::io::Result<(String, bool)> {
    let relative = relative_path(owner, device, agent);
    let path = absolute(workspace, &relative);
    if path.exists() {
        return Ok((relative, false));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // `create_new`, so a card that synced in between is never overwritten.
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            use std::io::Write;
            file.write_all(template(owner, device, agent).as_bytes())?;
            Ok((relative, true))
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok((relative, false)),
        Err(error) => Err(error),
    }
}

/// Every member device, from the control doc.
pub fn members(state: &AppState) -> Vec<MemberEntry> {
    let Ok(txn) = state.control.try_transact() else {
        return Vec::new();
    };
    let Some(map) = txn.get_map(MEMBER_LIST_KEY) else {
        return Vec::new();
    };
    map.iter(&txn)
        .filter_map(|(_, value)| match value {
            Out::Any(Any::String(raw)) => serde_json::from_str::<MemberEntry>(&raw).ok(),
            _ => None,
        })
        .collect()
}

/// Every agent the roster advertises, with its card where one exists.
///
/// `(member, agent, card)`, in roster order.
pub fn roster_cards(state: &AppState) -> Vec<(MemberEntry, String, Option<Card>)> {
    let mut out = Vec::new();
    for member in members(state) {
        if member.owner.is_empty() || member.device_label.is_empty() {
            continue;
        }
        for agent in &member.agents {
            let card = read(&state.workspace, &member.owner, &member.device_label, agent);
            out.push((member.clone(), agent.clone(), card));
        }
    }
    out
}

/// The card for one of this device's agents, or `None` when it has none or this
/// device does not yet know what the Circle calls it.
pub fn own(state: &AppState, agent: &str) -> Option<Card> {
    let me = state.self_member()?;
    read(&state.workspace, &me.owner, &me.device_label, agent)
}

/// One line describing a local agent, for the warden's choice of who answers.
///
/// Falls back to the bare name, so an agent without a card is still an option
/// rather than invisible.
pub fn local_summary(state: &AppState, agent: &str) -> String {
    match own(state, agent) {
        Some(card) if !card.summary.is_empty() => card.summary,
        _ => super::label::Roster::load(state).agent(agent, &state.peer_id),
    }
}

/// The part of the standing brief about cards: this agent's own, and one line
/// for each other agent that has said something about itself.
pub fn brief_section(state: &AppState, agent_id: &str) -> String {
    let me = state.self_member().filter(|m| !m.device_label.is_empty());
    let own_line = match &me {
        Some(me) => {
            let path = relative_path(&me.owner, &me.device_label, agent_id);
            match read(&state.workspace, &me.owner, &me.device_label, agent_id) {
                Some(card) if !card.summary.is_empty() || !card.body.trim().is_empty() => format!(
                    "Your card is {path}. It currently says:\n---\n{}\n---\nIf it is wrong about \
                     what you are or can do here, you may correct it while working on a request; \
                     the edit is reviewed like any other change. Do not edit another agent's \
                     card.\n",
                    clip(card.body.trim(), OWN_CARD_LIMIT)
                ),
                _ => format!(
                    "You have no card yet. If someone asks you to describe yourself, or you are \
                     working on a request and have a moment, you may write one at {path}: a \
                     one-line `summary` in front matter, then what model you are, what you can \
                     use here, and what you are good and not good for. The edit is reviewed like \
                     any other change.\n"
                ),
            }
        }
        None => String::new(),
    };
    let others: Vec<String> = roster_cards(state)
        .into_iter()
        .filter(|(member, agent, _)| {
            !(member.peer_id == state.peer_id && agent.eq_ignore_ascii_case(agent_id))
        })
        .filter_map(|(member, agent, card)| {
            let card = card.filter(|c| !c.summary.is_empty())?;
            Some(format!(
                "  @{}/{}/{} — {}",
                member.owner, member.device_label, agent, card.summary
            ))
        })
        .take(OTHERS_LIMIT)
        .collect();
    let others = if others.is_empty() {
        String::new()
    } else {
        format!(
            "What the other agents here say about themselves (their cards, under {DIR}/):\n{}\n",
            others.join("\n")
        )
    };
    if own_line.is_empty() && others.is_empty() {
        return String::new();
    }
    format!(
        "Each agent in this circle may describe itself in a card at \
         {DIR}/<owner>/<device>/<agent>.md.\n{own_line}{others}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_spells_the_handle() {
        assert_eq!(
            relative_path("suzy", "jessair", "claude"),
            "agents/suzy/jessair/claude.md"
        );
    }

    #[test]
    fn a_label_cannot_climb_out_of_its_folder() {
        assert_eq!(
            relative_path("../x", "a/b", ".hidden"),
            "agents/-x/a-b/hidden.md"
        );
        assert_eq!(relative_path("", " ", "c"), "agents/_/_/c.md");
    }

    #[test]
    fn an_untouched_template_says_nothing() {
        assert_eq!(summary_of(&template("suzy", "jessair", "claude")), "");
    }

    #[test]
    fn front_matter_summary_wins() {
        let text = "---\nsummary: \"Rust reviewer with a shell\"\n---\n# x\nSomething else.\n";
        assert_eq!(summary_of(text), "Rust reviewer with a shell");
    }

    #[test]
    fn without_a_summary_the_first_prose_line_is_used() {
        let text = "# @a/b/c\n<!-- guidance\nmore -->\n\n## Good for\n- Reading papers fast.\n";
        assert_eq!(summary_of(text), "Reading papers fast.");
    }

    #[test]
    fn a_long_summary_is_clipped() {
        let long = "x".repeat(400);
        let summary = summary_of(&format!("---\nsummary: {long}\n---\n"));
        assert_eq!(summary.chars().count(), SUMMARY_LIMIT + 1);
        assert!(summary.ends_with('…'));
    }

    fn state_with_roster(members: &[(&str, &str, &str, &[&str])]) -> (AppState, tempfile::TempDir) {
        use yrs::{Map, WriteTxn};
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(
            "c1".into(),
            "circle".into(),
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            String::new(),
            "label".into(),
            1,
            "local".into(),
            crate::config::JoinPolicy::Manual,
            "suzy".into(),
            crate::mls::new_mls_state(crate::mls::MlsIdentity::generate("local").unwrap(), None),
        );
        {
            let mut txn = state.control.transact_mut();
            let map = txn.get_or_insert_map(MEMBER_LIST_KEY);
            for (peer, owner, device, agents) in members {
                let entry = serde_json::json!({
                    "peer_id": peer, "owner": owner, "agent_id": owner,
                    "device_label": device, "agents": agents, "role": "member",
                    "added_at": "2026-10-01T00:00:00Z", "signature": "",
                });
                map.insert(&mut txn, *peer, Any::String(entry.to_string().into()));
            }
        }
        (state, dir)
    }

    #[test]
    fn the_brief_shows_your_card_and_the_others_summaries() {
        let (state, dir) = state_with_roster(&[
            ("local", "suzy", "jessair", &["claude"]),
            ("remote", "bob", "spark", &["codex", "gemini"]),
        ]);
        // No cards anywhere: the agent is told where its own would go, and
        // nothing about the others, who have said nothing.
        let brief = brief_section(&state, "claude");
        assert!(brief.contains("You have no card yet"), "{brief}");
        assert!(brief.contains("agents/suzy/jessair/claude.md"));
        assert!(!brief.contains("@bob/spark/codex"));

        std::fs::create_dir_all(dir.path().join("agents/bob/spark")).unwrap();
        std::fs::write(
            dir.path().join("agents/bob/spark/codex.md"),
            "---\nsummary: Runs the test suite on the GPU box\n---\n",
        )
        .unwrap();
        // An untouched template is not a description.
        create(dir.path(), "bob", "spark", "gemini").unwrap();
        create(dir.path(), "suzy", "jessair", "claude").unwrap();
        let brief = brief_section(&state, "claude");
        assert!(
            brief.contains("Your card is agents/suzy/jessair/claude.md"),
            "{brief}"
        );
        assert!(brief.contains("@bob/spark/codex — Runs the test suite on the GPU box"));
        assert!(!brief.contains("@bob/spark/gemini"));
        assert!(
            !brief.contains("@suzy/jessair/claude —"),
            "never listed as someone else"
        );
        assert_eq!(local_summary(&state, "claude"), "claude (on suzy/jessair)");
    }

    #[test]
    fn create_writes_the_template_once() {
        let dir = tempfile::tempdir().unwrap();
        let (path, created) = create(dir.path(), "suzy", "jessair", "claude").unwrap();
        assert!(created);
        assert_eq!(path, "agents/suzy/jessair/claude.md");
        let file = dir.path().join("agents/suzy/jessair/claude.md");
        std::fs::write(&file, "---\nsummary: mine\n---\n").unwrap();
        let (_, created) = create(dir.path(), "suzy", "jessair", "claude").unwrap();
        assert!(!created, "an existing card is never overwritten");
        assert_eq!(
            read(dir.path(), "suzy", "jessair", "claude")
                .unwrap()
                .summary,
            "mine"
        );
    }
}
