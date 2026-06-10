//! Daemon-local agent registry: maps agent names to launch command templates.
//!
//! The registry doubles as the allowlist — only registered agents may be
//! woken by a trigger. It lives in local daemon config and is never synced
//! across the circle, so a remote peer cannot force-enable an agent on this
//! device.
//!
//! ```toml
//! [agents.claude]
//! command = ["claude", "--print", "-p", "{{task}}"]
//!
//! [agents.codex]
//! command = ["codex", "{{task}}"]
//! ```

use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize)]
pub struct AgentCommand {
    /// Command and arguments; `{{task}}` is replaced with the mention's task
    /// text.
    pub command: Vec<String>,
    /// Working directory relative to the workspace root; defaults to the
    /// root itself.
    #[serde(default)]
    pub working_dir: Option<String>,
}

impl AgentCommand {
    /// Renders the command with the task text substituted. The task is
    /// passed as argv elements, never through a shell, so no quoting or
    /// injection concerns apply at this layer.
    pub fn render(&self, task: &str) -> Vec<String> {
        self.command
            .iter()
            .map(|part| part.replace("{{task}}", task))
            .collect()
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AgentRegistry {
    #[serde(default)]
    pub agents: BTreeMap<String, AgentCommand>,
}

impl AgentRegistry {
    pub fn from_toml(text: &str) -> anyhow::Result<Self> {
        Ok(toml::from_str(text)?)
    }

    /// The allowlist check: `None` means the trigger must be ignored.
    pub fn resolve(&self, agent: &str) -> Option<&AgentCommand> {
        self.agents.get(agent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"
        [agents.claude]
        command = ["claude", "--print", "-p", "{{task}}"]

        [agents.codex]
        command = ["codex", "{{task}}"]
        working_dir = "src"
    "#;

    #[test]
    fn parses_registry_and_renders_task() {
        let registry = AgentRegistry::from_toml(CONFIG).unwrap();
        let claude = registry.resolve("claude").unwrap();
        assert_eq!(
            claude.render("fix the sync docs"),
            vec!["claude", "--print", "-p", "fix the sync docs"]
        );
        assert_eq!(registry.resolve("codex").unwrap().working_dir.as_deref(), Some("src"));
    }

    #[test]
    fn unregistered_agent_is_not_resolved() {
        let registry = AgentRegistry::from_toml(CONFIG).unwrap();
        assert!(registry.resolve("openclaw").is_none());
    }

    #[test]
    fn empty_config_is_valid() {
        let registry = AgentRegistry::from_toml("").unwrap();
        assert!(registry.resolve("claude").is_none());
    }
}
