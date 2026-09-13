//! Daemon-local agent configuration: the allowlist, per-agent launch driver,
//! and the per-device reaction policy over chat mentions.
//!
//! This config is **never synced across the circle**. A remote member can
//! mention an agent, but only this device's local config decides whether — and
//! how — to react. That keeps execution authority local: a chat mention is
//! intent, not a command (see `docs/concepts/proposals.md`).
//!
//! Lives at `~/.enoxian/agents.toml`:
//!
//! ```toml
//! # How this device reacts to @mentions in any circle's chat.
//! reaction = "push"        # push = auto-run on mention; pull = do nothing
//!
//! [agents.claude]
//! driver = "acp"
//! command = ["<enoxian-home>/adapters/claude-agent-acp/<version>/node_modules/.bin/claude-agent-acp"]
//!
//! [agents.codex]
//! driver = "argv"          # default
//! command = ["codex", "{{task}}"]
//! engagement = "ambient"   # also read messages that name no agent
//! accept_from = "agents"   # let another agent delegate to this one
//! max_relay_turns = 20     # this device's ceiling on one cascade
//! ```

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// How a launched agent is driven once the daemon decides to run it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Driver {
    /// Fire-and-forget: substitute `{{task}}`, spawn, and let the ambient
    /// proposal engine capture whatever files the agent changed on disk.
    #[default]
    Argv,
    /// Speak the Agent Client Protocol over JSON-RPC/stdio. enoxian is the ACP
    /// client; the agent is the ACP agent. Gives a real turn lifecycle and, for
    /// agents that use client fs methods, per-write visibility.
    Acp,
}

/// How this device reacts to a chat mention of one of its agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reaction {
    /// The daemon subscribes to chat and auto-launches the mentioned agent.
    Push,
    /// The daemon initiates nothing; an agent is expected to retrieve chat and
    /// self-trigger. This is the safe default — no mention causes local
    /// execution unless the operator opts in.
    #[default]
    Pull,
}

/// Whether this agent reads the room, or only what is addressed to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engagement {
    /// Only explicit mentions and follow-ups. The default.
    #[default]
    Mention,
    /// Also offered every human message in the Circle, to answer or decline.
    ///
    /// Never a global switch and never a Circle-wide setting: the device that
    /// pays for an agent's tokens is the device that decides whether it reads
    /// idle chat. A synced property here would let a remote peer spend another
    /// device's budget.
    Ambient,
}

/// Whose mention may wake this agent on this device.
///
/// The switch lives on the *callee*, not the caller: the device that spends
/// the tokens decides whether another agent gets to spend them. There is
/// deliberately no sender-side opt-in — an agent's reply always carries its
/// relay chain, and whether that chain wakes anything is the receiving
/// device's call, under the receiving device's budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptFrom {
    /// Only human-authored mentions wake this agent. The safe default, and the
    /// answer a device that has never heard of delegation gives by
    /// construction.
    #[default]
    Humans,
    /// Another agent's mention may wake this one too, within the relay budget.
    Agents,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentCommand {
    /// Command and arguments. For the argv driver, `{{task}}` is replaced with
    /// the mention's task text. For the ACP driver the task is delivered in the
    /// prompt turn, so `{{task}}` is optional there.
    pub command: Vec<String>,
    #[serde(default)]
    pub driver: Driver,
    /// Working directory relative to the workspace root; defaults to the root.
    #[serde(default)]
    pub working_dir: Option<String>,
    /// Whether this agent is offered unaddressed messages. See [`Engagement`].
    #[serde(default)]
    pub engagement: Engagement,
    /// Whether another agent's mention may wake this one. See [`AcceptFrom`].
    #[serde(default)]
    pub accept_from: AcceptFrom,
    /// This device's ceiling on agent turns per delegation cascade. Clamps
    /// whatever a peer put on the wire; capped in turn by
    /// [`crate::agent::relay::RELAY_TURNS_CEILING`].
    #[serde(default = "default_max_relay_turns")]
    pub max_relay_turns: u8,
}

fn default_engagement_window() -> i64 {
    DEFAULT_ENGAGEMENT_WINDOW_SECS
}

fn default_max_relay_turns() -> u8 {
    crate::agent::relay::DEFAULT_MAX_RELAY_TURNS
}

impl Default for AgentCommand {
    fn default() -> Self {
        Self {
            command: Vec::new(),
            driver: Driver::default(),
            working_dir: None,
            engagement: Engagement::default(),
            accept_from: AcceptFrom::default(),
            max_relay_turns: default_max_relay_turns(),
        }
    }
}

impl AgentCommand {
    /// Carry this device's delegation settings over from a previous definition
    /// of the same agent. Editing an agent's command in the UI or CLI must not
    /// silently reset whether it accepts work from other agents.
    pub fn inheriting_delegation(mut self, previous: Option<&AgentCommand>) -> Self {
        if let Some(prev) = previous {
            self.engagement = prev.engagement;
            self.accept_from = prev.accept_from;
            self.max_relay_turns = prev.max_relay_turns;
        }
        self
    }

    /// Agents on this device that read the room rather than waiting to be
    /// addressed.
    pub fn is_ambient(&self) -> bool {
        self.engagement == Engagement::Ambient
    }
}

impl AgentCommand {
    /// Renders `command` with `{{task}}` substituted (used by the argv driver;
    /// harmless for ACP where the template is usually absent).
    pub fn render(&self, task: &str) -> Vec<String> {
        self.command
            .iter()
            .map(|part| part.replace("{{task}}", task))
            .collect()
    }
}

/// Default follow-up window: how long after an agent's reply a message from the
/// same person is routed back to it without a mention.
///
/// Three minutes is long enough to read a reply and type a considered answer,
/// short enough that a conversation abandoned mid-thread does not silently
/// capture an unrelated message later.
pub const DEFAULT_ENGAGEMENT_WINDOW_SECS: i64 = 180;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentConfig {
    #[serde(default)]
    pub reaction: Reaction,
    /// Seconds an agent stays "in conversation" with whoever it replied to, so
    /// a follow-up needs no mention. `0` disables follow-up routing entirely;
    /// every message then needs an explicit mention, as before.
    #[serde(default = "default_engagement_window")]
    pub engagement_window_secs: i64,
    #[serde(default)]
    pub agents: BTreeMap<String, AgentCommand>,
}

impl Default for AgentConfig {
    /// Matches the serde defaults field for field. A derived `Default` would
    /// give `engagement_window_secs = 0`, silently disabling follow-up routing
    /// on the fallback path while an empty `agents.toml` enabled it.
    fn default() -> Self {
        Self {
            reaction: Reaction::default(),
            engagement_window_secs: DEFAULT_ENGAGEMENT_WINDOW_SECS,
            agents: BTreeMap::new(),
        }
    }
}

impl AgentConfig {
    pub fn from_toml(text: &str) -> anyhow::Result<Self> {
        Ok(toml::from_str(text)?)
    }

    /// The allowlist check: `None` means no such agent is permitted here, so a
    /// mention of it must be ignored.
    pub fn resolve(&self, agent: &str) -> Option<&AgentCommand> {
        self.agents.get(agent)
    }

    /// Path to the daemon-local agent config file.
    pub fn path() -> anyhow::Result<PathBuf> {
        Ok(crate::config::enoxian_dir()?.join("agents.toml"))
    }

    /// Load `~/.enoxian/agents.toml`, or an empty (pull, no agents) config if it
    /// is missing or unparseable. Missing config = the device reacts to nothing,
    /// which is the safe default.
    pub fn load() -> Self {
        let Ok(path) = Self::path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => match Self::from_toml(&text) {
                Ok(cfg) => cfg,
                Err(e) => {
                    tracing::warn!(
                        "[agent] {} is invalid ({e}); reacting to nothing",
                        path.display()
                    );
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    /// Load the config for editing. Unlike [`load`], a parse error here is a
    /// hard failure rather than a silent default — we must not overwrite an
    /// unparseable file the user is mid-editing and clobber their work.
    pub fn load_for_edit() -> anyhow::Result<Self> {
        let path = Self::path()?;
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::from_toml(&text).map_err(|e| {
                anyhow::anyhow!(
                    "{} is not valid TOML ({e}); fix it by hand first",
                    path.display()
                )
            }),
            // Missing file is fine — start from an empty config.
            Err(_) => Ok(Self::default()),
        }
    }

    /// Write the config back to `agents.toml`.
    ///
    /// Note: this serializes the struct, so any comments in a hand-edited file
    /// are dropped. That is an accepted trade-off for programmatic editing; the
    /// values are preserved exactly.
    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let toml = toml::to_string_pretty(self)?;
        std::fs::write(&path, toml)?;
        Ok(())
    }

    /// Add or replace an agent. `driver`/`command` fully define how it launches.
    pub fn set_agent(&mut self, name: &str, cmd: AgentCommand) {
        self.agents.insert(name.to_string(), cmd);
    }

    /// Remove an agent. Returns true if it existed.
    pub fn remove_agent(&mut self, name: &str) -> bool {
        self.agents.remove(name).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"
        reaction = "push"

        [agents.claude]
        driver = "acp"
        command = ["claude-agent-acp"]

        [agents.codex]
        command = ["codex", "{{task}}"]
        working_dir = "src"
    "#;

    #[test]
    fn engagement_settings_round_trip_through_toml() {
        // What the settings UI writes must survive a save/load cycle, or a
        // change appears to stick and silently does not.
        let text = r#"
reaction = "push"
engagement_window_secs = 90

[agents.claude]
driver = "acp"
command = ["claude-agent-acp"]
engagement = "ambient"
accept_from = "agents"
max_relay_turns = 6
"#;
        let cfg = AgentConfig::from_toml(text).unwrap();
        assert_eq!(cfg.engagement_window_secs, 90);
        let claude = cfg.resolve("claude").unwrap();
        assert_eq!(claude.engagement, Engagement::Ambient);
        assert_eq!(claude.accept_from, AcceptFrom::Agents);
        assert_eq!(claude.max_relay_turns, 6);

        let back = AgentConfig::from_toml(&toml::to_string_pretty(&cfg).unwrap()).unwrap();
        let claude = back.resolve("claude").unwrap();
        assert_eq!(back.engagement_window_secs, 90);
        assert_eq!(claude.engagement, Engagement::Ambient);
        assert_eq!(claude.accept_from, AcceptFrom::Agents);
        assert_eq!(claude.max_relay_turns, 6);
    }

    #[test]
    fn an_agent_without_engagement_keys_gets_the_safe_defaults() {
        // A config written before these existed must not silently opt an agent
        // into reading the room or accepting delegation.
        let text = r#"
reaction = "push"

[agents.claude]
driver = "acp"
command = ["claude-agent-acp"]
"#;
        let cfg = AgentConfig::from_toml(text).unwrap();
        let claude = cfg.resolve("claude").unwrap();
        assert_eq!(claude.engagement, Engagement::Mention);
        assert_eq!(claude.accept_from, AcceptFrom::Humans);
        assert_eq!(
            claude.max_relay_turns,
            crate::agent::relay::DEFAULT_MAX_RELAY_TURNS
        );
        assert_eq!(cfg.engagement_window_secs, DEFAULT_ENGAGEMENT_WINDOW_SECS);
    }

    #[test]
    fn parses_config_with_drivers_and_reaction() {
        let cfg = AgentConfig::from_toml(CONFIG).unwrap();
        assert_eq!(cfg.reaction, Reaction::Push);

        let claude = cfg.resolve("claude").unwrap();
        assert_eq!(claude.driver, Driver::Acp);
        assert_eq!(claude.command, vec!["claude-agent-acp"]);

        let codex = cfg.resolve("codex").unwrap();
        assert_eq!(codex.driver, Driver::Argv, "driver defaults to argv");
        assert_eq!(codex.render("fix docs"), vec!["codex", "fix docs"]);
        assert_eq!(codex.working_dir.as_deref(), Some("src"));
    }

    #[test]
    fn edit_roundtrips_through_toml() {
        let mut cfg = AgentConfig {
            reaction: Reaction::Push,
            ..Default::default()
        };
        cfg.set_agent(
            "claude",
            AgentCommand {
                command: vec!["claude-agent-acp".into()],
                driver: Driver::Acp,
                ..Default::default()
            },
        );
        // Serialize and reparse — values survive a save/load cycle.
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back = AgentConfig::from_toml(&text).unwrap();
        assert_eq!(back.reaction, Reaction::Push);
        assert_eq!(back.resolve("claude").unwrap().driver, Driver::Acp);

        // Removal.
        let mut cfg2 = back;
        assert!(cfg2.remove_agent("claude"));
        assert!(!cfg2.remove_agent("claude"));
        assert!(cfg2.resolve("claude").is_none());
    }

    #[test]
    fn empty_config_is_pull_and_reacts_to_nothing() {
        let cfg = AgentConfig::from_toml("").unwrap();
        assert_eq!(cfg.reaction, Reaction::Pull);
        assert!(cfg.resolve("claude").is_none());
    }

    #[test]
    fn unregistered_agent_is_not_resolved() {
        let cfg = AgentConfig::from_toml(CONFIG).unwrap();
        assert!(cfg.resolve("openclaw").is_none());
    }
}
