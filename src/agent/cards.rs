//! Agent cards: what each agent in a Circle says it is good for.
//!
//! A card is a Markdown file in the shared folder, one per agent per device, at
//! `agents/<owner>/<device>/<agent>.md` — the path spells the agent's handle, so
//! the card for `@suzy/jessair/claude` is `agents/suzy/jessair/claude.md`. Per
//! device rather than per name, because two `claude`s on two machines can run
//! different models with different tools.
//!
//! Each device writes its own agents' cards (`ensure`), so nobody makes one by
//! hand and no two devices write the same file. A card opens with a facts
//! block the device keeps current — what the agent is and runs, as it reported
//! over ACP, and whether it reads the room — and the rest is free text for the
//! agent and the people working with it.
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
use yrs::{Any, GetString, Map, Out, ReadTxn, Text, Transact, WriteTxn};

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
    let (prose, facts) = split_facts(text);
    let line = prose_summary(&prose).or_else(|| facts_summary(facts));
    line.map(|l| clip(&l.replace(['\r', '\n'], " "), SUMMARY_LIMIT))
        .unwrap_or_default()
}

/// The card without its facts block, and the block's lines.
fn split_facts(text: &str) -> (String, Option<&str>) {
    match (text.find(FACTS_START), text.find(FACTS_END)) {
        (Some(start), Some(end)) if end > start => (
            format!("{}{}", &text[..start], &text[end + FACTS_END.len()..]),
            Some(&text[start + FACTS_START.len()..end]),
        ),
        _ => (text.to_string(), None),
    }
}

/// What the facts say the agent is, for a card nobody has summarized:
/// "Claude Agent 0.69.0, Opus".
fn facts_summary(facts: Option<&str>) -> Option<String> {
    let parts: Vec<&str> = facts?
        .lines()
        .filter_map(|line| {
            let line = line.trim().trim_start_matches("- ");
            line.strip_prefix("Agent: ")
                .or_else(|| line.strip_prefix("Model: "))
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

fn prose_summary(text: &str) -> Option<String> {
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
    front_summary.or_else(|| {
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
    })
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

/// What an agent said about itself over ACP, kept so its card can show it.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Reported {
    /// The product's own name for itself, e.g. "Claude Agent".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The adapter package, e.g. "@agentclientprotocol/claude-agent-acp".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The model the agent's session runs, as the agent names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl Reported {
    /// Read `agentInfo` from an `initialize` result.
    pub fn read_agent_info(&mut self, result: &serde_json::Value) {
        let info = &result["agentInfo"];
        let text = |key: &str| {
            info[key]
                .as_str()
                .map(str::to_string)
                .filter(|s| !s.is_empty())
        };
        self.title = text("title").or(self.title.take());
        self.product = text("name").or(self.product.take());
        self.version = text("version").or(self.version.take());
    }

    /// Read the session's model from a `session/new` or `session/load` result.
    ///
    /// Agents report it two ways: a `configOptions` entry in the `model`
    /// category (Claude's adapter), or a `models` block (Codex's). Either names
    /// the current value and, usually, a display name for it.
    pub fn read_session(&mut self, result: &serde_json::Value) {
        let from_options = result["configOptions"].as_array().and_then(|options| {
            let option = options
                .iter()
                .find(|o| o["category"] == "model" || o["id"] == "model")?;
            let current = option["currentValue"].as_str()?;
            let name = option["options"].as_array().and_then(|values| {
                values
                    .iter()
                    .find(|v| v["value"] == current)
                    .and_then(|v| v["name"].as_str())
            });
            Some(name.unwrap_or(current).to_string())
        });
        let from_models = || {
            let models = &result["models"];
            let current = models["currentModelId"].as_str()?;
            let name = models["availableModels"].as_array().and_then(|list| {
                list.iter()
                    .find(|m| m["modelId"] == current)
                    .and_then(|m| m["name"].as_str())
            });
            Some(name.unwrap_or(current).to_string())
        };
        if let Some(model) = from_options.or_else(from_models) {
            self.model = Some(model);
        }
    }

    fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

fn reported_path(circle_dir: &Path, agent: &str) -> PathBuf {
    circle_dir
        .join("agents-reported")
        .join(format!("{}.json", segment(agent)))
}

/// Keep what an agent reported, if it says anything new. Best-effort: a card
/// that misses a model name is not worth failing a turn over.
pub fn record_reported(circle_dir: &Path, agent: &str, reported: &Reported) {
    if reported.is_empty() || load_reported(circle_dir, agent).as_ref() == Some(reported) {
        return;
    }
    let path = reported_path(circle_dir, agent);
    let result = path
        .parent()
        .map(std::fs::create_dir_all)
        .transpose()
        .and_then(|_| {
            std::fs::write(
                &path,
                serde_json::to_vec_pretty(reported).unwrap_or_default(),
            )
        });
    if let Err(error) = result {
        tracing::debug!("[cards] could not keep what {agent} reported: {error}");
    }
}

pub fn load_reported(circle_dir: &Path, agent: &str) -> Option<Reported> {
    let text = std::fs::read_to_string(reported_path(circle_dir, agent)).ok()?;
    serde_json::from_str(&text).ok()
}

/// What this device knows about one of its agents, for the card's facts block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    pub owner: String,
    pub device: String,
    pub agent: String,
    /// `acp` or `argv`; `None` for an agent advertised without a launcher here.
    pub driver: Option<String>,
    /// The installed adapter and its version, read off its launch command.
    pub adapter: Option<String>,
    /// Whether it reads the room in this Circle.
    pub scries: bool,
    pub reported: Option<Reported>,
}

/// The adapter a launch command runs: `claude-agent-acp 0.69.0` for one
/// installed under `adapters/<id>/<version>/`, else the program's own name.
fn adapter_of(command: &[String]) -> Option<String> {
    let program = command.first()?;
    let parts: Vec<&str> = program.split(['/', '\\']).collect();
    if let Some(at) = parts.iter().position(|p| *p == "adapters") {
        if let (Some(id), Some(version)) = (parts.get(at + 1), parts.get(at + 2)) {
            return Some(format!("{id} {version}"));
        }
    }
    parts
        .last()
        .filter(|p| !p.is_empty())
        .map(|p| p.to_string())
}

/// What this device knows about `agent` as it runs here.
pub fn facts_for(
    state: &AppState,
    cfg: &super::config::AgentConfig,
    owner: &str,
    device: &str,
    agent: &str,
) -> Facts {
    let command = cfg.resolve(agent);
    Facts {
        owner: owner.to_string(),
        device: device.to_string(),
        agent: agent.to_string(),
        driver: command.map(|c| format!("{:?}", c.driver).to_lowercase()),
        adapter: command.and_then(|c| adapter_of(&c.command)),
        scries: cfg.resolved(&state.circle_id).is_ambient(agent),
        reported: load_reported(&state.circle_dir, agent),
    }
}

const FACTS_START: &str =
    "<!-- enoxian:facts — kept up to date by enoxian; edits here are replaced -->";
const FACTS_END: &str = "<!-- /enoxian:facts -->";

/// The facts block: what the device running the agent knows, in plain lines
/// a reader and the warden can both use. The first line doubles as the
/// card's summary until someone writes one.
pub fn facts_block(facts: &Facts) -> String {
    let reported = facts.reported.clone().unwrap_or_default();
    let mut lines = Vec::new();
    let product = match (&reported.title, &reported.version) {
        (Some(title), Some(version)) => Some(format!("{title} {version}")),
        (Some(title), None) => Some(title.clone()),
        _ => facts.adapter.clone(),
    };
    if let Some(product) = product {
        lines.push(format!("- Agent: {product}"));
    }
    if let Some(model) = &reported.model {
        lines.push(format!("- Model: {model}"));
    }
    let how = match (&facts.driver, &facts.adapter) {
        (Some(driver), Some(adapter)) => format!(", through {} ({adapter})", driver.to_uppercase()),
        (Some(driver), None) => format!(", through {}", driver.to_uppercase()),
        _ => String::new(),
    };
    lines.push(format!("- Runs on: {}/{}{how}", facts.owner, facts.device));
    lines.push(format!(
        "- Reads the room here: {}",
        if facts.scries {
            "yes"
        } else {
            "no, answers when named"
        }
    ));
    format!("{FACTS_START}\n{}\n{FACTS_END}", lines.join("\n"))
}

/// A new card: its facts, and empty sections for what only the agent or the
/// people working with it can say. Guidance is in comments, so an untouched
/// card summarizes to its first fact rather than to the instructions.
pub fn template(facts: &Facts) -> String {
    format!(
        "---\n\
         summary: \"\"\n\
         ---\n\
         # @{owner}/{device}/{agent}\n\
         \n\
         {facts}\n\
         \n\
         <!-- One line in `summary` above is what other agents and the warden see first.\n\
         Describe what this agent is, not how well it has done: outcomes are measured, not\n\
         written here. Edits are reviewed like any other change. -->\n\
         \n\
         ## Can use\n\
         <!-- Tools it has here: shell, web, files, tests, a particular checkout. -->\n\
         \n\
         ## Good for\n\
         <!-- The questions and work to send its way. -->\n\
         \n\
         ## Not for\n\
         <!-- What to ask someone else instead. -->\n",
        owner = facts.owner,
        device = facts.device,
        agent = facts.agent,
        facts = facts_block(facts),
    )
}

/// `text` with its facts block brought up to date, or `None` if it already
/// is. A card without a block, written by hand, gets one under its heading.
pub fn refresh(text: &str, facts: &Facts) -> Option<String> {
    let block = facts_block(facts);
    let updated = match (text.find(FACTS_START), text.find(FACTS_END)) {
        (Some(start), Some(end)) if end > start => {
            format!(
                "{}{block}{}",
                &text[..start],
                &text[end + FACTS_END.len()..]
            )
        }
        _ => {
            // Under the first heading, else after front matter, else on top.
            let at = text
                .find("\n# ")
                .and_then(|h| text[h + 1..].find('\n').map(|e| h + 1 + e + 1))
                .unwrap_or(0);
            format!("{}\n{block}\n\n{}", &text[..at], &text[at..]).replacen("\n\n\n", "\n\n", 1)
        }
    };
    (updated != text).then_some(updated)
}

/// Write or refresh the card of every agent this device advertises here.
///
/// Each device writes only its own agents' cards, so no two devices ever
/// write the same file. A card is created once and afterwards only its facts
/// block is rewritten, and only when a fact changed; everything else in it
/// belongs to the agent and the people working with it. Returns the paths
/// written.
pub fn ensure(state: &AppState, cfg: &super::config::AgentConfig) -> Vec<String> {
    let Some(me) = state.self_member().filter(|m| !m.device_label.is_empty()) else {
        return Vec::new();
    };
    let mut written = Vec::new();
    for agent in &me.agents {
        let facts = facts_for(state, cfg, &me.owner, &me.device_label, agent);
        let relative = relative_path(&me.owner, &me.device_label, agent);
        match write_card(state, &relative, &facts) {
            Ok(true) => written.push(relative),
            Ok(false) => {}
            Err(error) => tracing::warn!("[cards] could not write {relative}: {error}"),
        }
    }
    written
}

/// Create or refresh one card, through the shared document first and then the
/// file, the way the file API creates files. The document is the live copy: a
/// card open in someone's editor may have edits the file has not caught up
/// with, and only the facts block is touched in it, so those survive.
fn write_card(state: &AppState, relative: &str, facts: &Facts) -> std::io::Result<bool> {
    let path = absolute(&state.workspace, relative);
    let on_disk = match std::fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let doc = state.get_or_create_doc(relative);
    let content = {
        let Ok(mut txn) = doc.try_transact_mut() else {
            return Ok(false); // busy; the next pass gets it
        };
        let text = txn.get_or_insert_text(relative);
        let current = text.get_string(&txn);
        let current = if current.is_empty() {
            on_disk.clone()
        } else {
            Some(current)
        };
        let next = match &current {
            None => template(facts),
            Some(current) => match refresh(current, facts) {
                Some(next) => next,
                None if on_disk.as_ref() == Some(current) => return Ok(false),
                None => current.clone(), // the file is behind; let it catch up
            },
        };
        let had = text.get_string(&txn);
        if had != next {
            let (start, removed, inserted) = changed_span(&had, &next);
            if removed > 0 {
                text.remove_range(&mut txn, start, removed);
            }
            if !inserted.is_empty() {
                text.insert(&mut txn, start, inserted);
            }
        }
        next
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, &content)?;
    let _ = state.events.send(crate::control::CircleEvent::FileUpdated {
        path: relative.to_string(),
    });
    Ok(true)
}

/// The one span where `next` differs from `had`: where it starts, how much of
/// `had` it removes and what it inserts. Offsets in the units `yrs` counts text
/// in (UTF-8 bytes, its default), cut on character boundaries.
fn changed_span<'a>(had: &str, next: &'a str) -> (u32, u32, &'a str) {
    let prefix = had
        .char_indices()
        .zip(next.chars())
        .find(|((_, a), b)| a != b)
        .map(|((i, _), _)| i)
        .unwrap_or(had.len().min(next.len()));
    let prefix = (0..=prefix)
        .rev()
        .find(|&i| had.is_char_boundary(i) && next.is_char_boundary(i))
        .unwrap_or(0);
    let (had_tail, next_tail) = (&had[prefix..], &next[prefix..]);
    let mut suffix = had_tail
        .bytes()
        .rev()
        .zip(next_tail.bytes().rev())
        .take_while(|(a, b)| a == b)
        .count();
    while suffix > 0
        && !(had_tail.is_char_boundary(had_tail.len() - suffix)
            && next_tail.is_char_boundary(next_tail.len() - suffix))
    {
        suffix -= 1;
    }
    (
        prefix as u32,
        (had_tail.len() - suffix) as u32,
        &next_tail[..next_tail.len() - suffix],
    )
}

#[cfg(test)]
fn test_facts(owner: &str, device: &str, agent: &str) -> Facts {
    Facts {
        owner: owner.into(),
        device: device.into(),
        agent: agent.into(),
        driver: None,
        adapter: None,
        scries: false,
        reported: None,
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
                    "Your card is {path}. It currently says:\n---\n{}\n---\nThe block between \
                     the enoxian:facts markers is kept up to date by this device; leave it be. \
                     The rest is yours: if it is empty or wrong about what you can use here or \
                     are good for, you may fill it in or correct it while working on a request, \
                     and the edit is reviewed like any other change. Do not edit another agent's \
                     card.\n",
                    clip(card.body.trim(), OWN_CARD_LIMIT)
                ),
                _ => format!(
                    "Your card, at {path}, has not been written yet; this device writes it \
                     shortly.\n"
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
        assert_eq!(
            summary_of(&template(&test_facts("suzy", "jessair", "claude"))),
            ""
        );
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
        assert!(brief.contains("has not been written yet"), "{brief}");
        assert!(brief.contains("agents/suzy/jessair/claude.md"));
        assert!(!brief.contains("@bob/spark/codex"));

        std::fs::create_dir_all(dir.path().join("agents/bob/spark")).unwrap();
        std::fs::write(
            dir.path().join("agents/bob/spark/codex.md"),
            "---\nsummary: Runs the test suite on the GPU box\n---\n",
        )
        .unwrap();
        // An untouched template is not a description.
        write_template(dir.path(), "bob", "spark", "gemini");
        write_template(dir.path(), "suzy", "jessair", "claude");
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

    fn write_template(workspace: &Path, owner: &str, device: &str, agent: &str) {
        let path = absolute(workspace, &relative_path(owner, device, agent));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, template(&test_facts(owner, device, agent))).unwrap();
    }

    #[test]
    fn what_claude_reports_reads_as_product_and_model() {
        let mut reported = Reported::default();
        reported.read_agent_info(&serde_json::json!({
            "protocolVersion": 1,
            "agentInfo": {
                "name": "@agentclientprotocol/claude-agent-acp",
                "title": "Claude Agent",
                "version": "0.69.0"
            }
        }));
        reported.read_session(&serde_json::json!({
            "sessionId": "s",
            "configOptions": [
                {"id": "mode", "category": "mode", "currentValue": "default", "options": []},
                {"id": "model", "category": "model", "currentValue": "opus",
                 "options": [{"value": "sonnet", "name": "Sonnet"},
                             {"value": "opus", "name": "Opus"}]}
            ]
        }));
        assert_eq!(reported.title.as_deref(), Some("Claude Agent"));
        assert_eq!(reported.version.as_deref(), Some("0.69.0"));
        assert_eq!(reported.model.as_deref(), Some("Opus"));
    }

    #[test]
    fn a_models_block_names_the_model_when_there_are_no_options() {
        let mut reported = Reported::default();
        reported.read_session(&serde_json::json!({
            "models": {
                "currentModelId": "gpt-5.5/high",
                "availableModels": [{"modelId": "gpt-5.5/high", "name": "GPT-5.5 (high)"}]
            }
        }));
        assert_eq!(reported.model.as_deref(), Some("GPT-5.5 (high)"));
        // A session that says nothing about its model keeps the last one known.
        reported.read_session(&serde_json::json!({"sessionId": "s"}));
        assert_eq!(reported.model.as_deref(), Some("GPT-5.5 (high)"));
    }

    #[test]
    fn the_adapter_is_read_off_its_install_path() {
        let command = |p: &str| vec![p.to_string(), "--flag".to_string()];
        assert_eq!(
            adapter_of(&command(
                "/Users/s/.enoxian/adapters/claude-agent-acp/0.69.0/node_modules/.bin/claude-agent-acp"
            ))
            .as_deref(),
            Some("claude-agent-acp 0.69.0")
        );
        assert_eq!(
            adapter_of(&command("/usr/local/bin/codex-acp")).as_deref(),
            Some("codex-acp")
        );
        assert_eq!(adapter_of(&[]), None);
    }

    fn claude_facts() -> Facts {
        Facts {
            driver: Some("acp".into()),
            adapter: Some("claude-agent-acp 0.69.0".into()),
            scries: true,
            reported: Some(Reported {
                title: Some("Claude Agent".into()),
                product: None,
                version: Some("0.69.0".into()),
                model: Some("Opus".into()),
            }),
            ..test_facts("suzy", "jessair", "claude")
        }
    }

    #[test]
    fn an_unsummarized_card_is_summarized_by_its_facts() {
        let text = template(&claude_facts());
        assert!(text.contains("- Runs on: suzy/jessair, through ACP (claude-agent-acp 0.69.0)\n"));
        assert!(text.contains("- Reads the room here: yes\n"));
        assert_eq!(summary_of(&text), "Claude Agent 0.69.0, Opus");
        // Anything written by a person or the agent wins over the facts.
        let written = text.replace("## Good for\n", "## Good for\n- Reviewing Rust.\n");
        assert_eq!(summary_of(&written), "Reviewing Rust.");
    }

    #[test]
    fn refresh_rewrites_the_facts_and_nothing_else() {
        let before = template(&test_facts("suzy", "jessair", "claude"))
            .replace("## Good for\n", "## Good for\n- Reviewing Rust.\n");
        let after = refresh(&before, &claude_facts()).expect("facts changed");
        assert!(after.contains("- Model: Opus"));
        assert!(after.contains("- Reviewing Rust.\n"));
        assert_eq!(
            after.replace(&facts_block(&claude_facts()), ""),
            before.replace(&facts_block(&test_facts("suzy", "jessair", "claude")), "")
        );
        assert_eq!(
            refresh(&after, &claude_facts()),
            None,
            "nothing to do twice"
        );

        // A card written by hand, before cards had facts, gets them under its
        // heading and keeps everything it said.
        let by_hand = "---\nsummary: mine\n---\n# @suzy/jessair/claude\nWrites papers.\n";
        let with = refresh(by_hand, &claude_facts()).unwrap();
        assert!(
            with.starts_with(
                "---\nsummary: mine\n---\n# @suzy/jessair/claude\n\n<!-- enoxian:facts"
            ),
            "{with}"
        );
        assert!(
            with.ends_with("<!-- /enoxian:facts -->\n\nWrites papers.\n"),
            "{with}"
        );
        assert_eq!(summary_of(&with), "mine");
    }

    #[test]
    fn the_changed_span_is_cut_on_character_boundaries() {
        let had = "前 — 名片 a";
        let next = "前 — 名牌 a";
        let (start, removed, inserted) = changed_span(had, next);
        let (start, removed) = (start as usize, removed as usize);
        assert_eq!(
            format!("{}{}{}", &had[..start], inserted, &had[start + removed..]),
            next
        );
        assert_eq!(changed_span("abc", "abc"), (3, 0, ""));
        assert_eq!(changed_span("", "x"), (0, 0, "x"));
    }

    #[test]
    fn ensure_writes_each_card_once_then_only_its_facts() {
        let (state, dir) = state_with_roster(&[("local", "suzy", "jessair", &["claude"])]);
        let cfg = super::super::config::AgentConfig::from_toml("").unwrap();
        let path = "agents/suzy/jessair/claude.md";
        assert_eq!(ensure(&state, &cfg), vec![path.to_string()]);
        assert_eq!(
            ensure(&state, &cfg),
            Vec::<String>::new(),
            "nothing changed"
        );

        // Someone fills in a section in the editor; the file has not caught up.
        let doc = state.get_or_create_doc(path);
        {
            let mut txn = doc.transact_mut();
            let text = txn.get_or_insert_text(path);
            let current = text.get_string(&txn);
            let at = current.find("## Good for\n").unwrap() + "## Good for\n".len();
            text.insert(&mut txn, at as u32, "- Reviewing Rust.\n");
        }
        // Then the agent reports a model.
        record_reported(
            &state.circle_dir,
            "claude",
            &Reported {
                model: Some("Opus".into()),
                ..Default::default()
            },
        );
        assert_eq!(ensure(&state, &cfg), vec![path.to_string()]);
        let on_disk = std::fs::read_to_string(dir.path().join(path)).unwrap();
        let in_doc = {
            let txn = doc.transact();
            txn.get_text(path).unwrap().get_string(&txn)
        };
        assert_eq!(on_disk, in_doc);
        assert!(on_disk.contains("- Model: Opus\n"), "{on_disk}");
        assert!(on_disk.contains("- Reviewing Rust.\n"), "the edit survives");
        assert_eq!(ensure(&state, &cfg), Vec::<String>::new());
    }
}
