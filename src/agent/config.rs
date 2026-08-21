//! Daemon-local agent configuration: the allowlist, per-agent launch driver,
//! and the per-device reaction policy over chat mentions.
//!
//! This config is **never synced across the circle**. A remote member can
//! mention an agent, but only this device's local config decides whether — and
//! how — to react. That keeps execution authority local: a chat mention is
//! intent, not a command (see `docs/plan/agent-workspaces.md` → Two-Layer
//! Split).
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

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AgentConfig {
    #[serde(default)]
    pub reaction: Reaction,
    #[serde(default)]
    pub agents: BTreeMap<String, AgentCommand>,
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
                working_dir: None,
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
