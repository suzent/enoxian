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

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
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
    /// Read from configs written before engagement settings were scoped, and
    /// folded into the global scope on load. Never written back.
    ///
    /// Behaviour moved out of `[agents.*]` because it is a property of *where*
    /// an agent is working, not of how it is launched — the same `claude`
    /// binary should be able to read the room in one Circle and not another.
    #[serde(default, skip_serializing)]
    pub engagement: Option<Engagement>,
    /// Legacy, ignored. Delegation is no longer opt-in per agent: an agent you
    /// have already allowed into a Circle is reachable by the other agents in
    /// it. See [`EngagementSettings::max_relay_turns`] for the bound that
    /// replaced the switch.
    #[serde(default, skip_serializing)]
    pub accept_from: Option<AcceptFrom>,
    /// Legacy, folded into the global scope on load.
    #[serde(default, skip_serializing)]
    pub max_relay_turns: Option<u8>,
}

fn default_engagement_window() -> i64 {
    DEFAULT_ENGAGEMENT_WINDOW_SECS
}

fn default_max_relay_turns() -> u8 {
    crate::agent::relay::DEFAULT_MAX_RELAY_TURNS
}

impl AgentCommand {
    /// Carry this device's delegation settings over from a previous definition
    /// of the same agent. Editing an agent's command in the UI or CLI must not
    /// silently reset whether it accepts work from other agents.
    pub fn inheriting_delegation(self, _previous: Option<&AgentCommand>) -> Self {
        // Nothing per-agent left to carry: engagement settings live in their
        // own scopes now, which editing a launch command never touches.
        self
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
/// Explicit reply threads are the default. Existing configured windows remain
/// opt-in behavior and are not overwritten during migration.
pub const DEFAULT_ENGAGEMENT_WINDOW_SECS: i64 = 0;

/// How agents behave in one scope — globally, or in a single Circle.
///
/// Every field is optional at Circle scope, where `None` means "inherit the
/// global answer". The global scope fills each in with a default, so there is
/// always a concrete answer at the bottom.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct EngagementSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ambient_responders: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ambient_rotate_count: Option<bool>,
    /// How this device reacts to mentions here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reaction: Option<Reaction>,
    /// Seconds an agent stays "in conversation" with whoever it replied to, so
    /// a follow-up needs no mention. `0` disables follow-up routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engagement_window_secs: Option<i64>,
    /// Agents that read every human message here rather than waiting to be
    /// addressed. Named rather than flagged per agent, because whether an agent
    /// reads the room is a property of the room.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ambient: Option<Vec<String>>,
    /// Ceiling on agent turns in one delegation chain. Clamps whatever a peer
    /// put on the wire; capped in turn by
    /// [`crate::agent::relay::RELAY_TURNS_CEILING`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_relay_turns: Option<u8>,
}

/// Settings with every question answered — what a caller actually works with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSettings {
    pub ambient_responders: usize,
    pub ambient_rotate_count: bool,
    pub reaction: Reaction,
    pub engagement_window_secs: i64,
    pub ambient: Vec<String>,
    pub max_relay_turns: u8,
}

impl ResolvedSettings {
    /// Does this agent read the room here?
    pub fn is_ambient(&self, agent: &str) -> bool {
        self.ambient.iter().any(|a| a.eq_ignore_ascii_case(agent))
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentConfig {
    #[serde(default = "default_ambient_responders")]
    pub ambient_responders: usize,
    #[serde(default)]
    pub ambient_rotate_count: bool,
    /// Device-wide execution cap. Set to 1 for serial rollback; restart to resize.
    #[serde(default = "default_max_concurrent_runs")]
    pub max_concurrent_runs: usize,
    #[serde(default)]
    pub reaction: Reaction,
    /// Seconds an agent stays "in conversation" with whoever it replied to, so
    /// a follow-up needs no mention. `0` disables follow-up routing entirely;
    /// every message then needs an explicit mention, as before.
    #[serde(default = "default_engagement_window")]
    pub engagement_window_secs: i64,
    /// Agents that read the room in every Circle unless a Circle says
    /// otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ambient: Vec<String>,
    /// The global ceiling on a delegation chain.
    #[serde(default = "default_max_relay_turns")]
    pub max_relay_turns: u8,
    /// Per-Circle overrides, keyed by circle id. Anything absent inherits the
    /// global answer above.
    ///
    /// Device-local like the rest of this file. A synced per-Circle setting
    /// would let a remote member decide what this machine spends, which is the
    /// one thing the execution model does not allow.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub circles: BTreeMap<String, EngagementSettings>,
    #[serde(default)]
    pub agents: BTreeMap<String, AgentCommand>,
}

impl AgentConfig {
    /// The settings that apply in one Circle: the global answers, with any
    /// Circle-scoped overrides laid on top.
    pub fn resolved(&self, circle_id: &str) -> ResolvedSettings {
        let over = self.circles.get(circle_id);
        ResolvedSettings {
            ambient_responders: over
                .and_then(|o| o.ambient_responders)
                .unwrap_or(self.ambient_responders)
                .clamp(1, 32),
            ambient_rotate_count: over
                .and_then(|o| o.ambient_rotate_count)
                .unwrap_or(self.ambient_rotate_count),
            reaction: over.and_then(|o| o.reaction).unwrap_or(self.reaction),
            engagement_window_secs: over
                .and_then(|o| o.engagement_window_secs)
                .unwrap_or(self.engagement_window_secs),
            ambient: over
                .and_then(|o| o.ambient.clone())
                .unwrap_or_else(|| self.ambient.clone()),
            max_relay_turns: over
                .and_then(|o| o.max_relay_turns)
                .unwrap_or(self.max_relay_turns),
        }
    }

    /// Fold settings written under `[agents.*]` before they were scoped into
    /// the global scope, so an existing config keeps behaving the same way.
    ///
    /// `accept_from` is deliberately dropped rather than migrated: delegation
    /// is no longer opt-in, so there is nothing for it to mean. The legacy
    /// fields are never written back, so a save quietly completes the move.
    fn migrate_legacy_agent_settings(&mut self) {
        for (name, cmd) in &self.agents {
            if cmd.engagement == Some(Engagement::Ambient)
                && !self.ambient.iter().any(|a| a.eq_ignore_ascii_case(name))
            {
                self.ambient.push(name.clone());
            }
        }
        // A per-agent cap becomes the global one. Take the smallest, so
        // migrating never raises a ceiling somebody had lowered.
        if let Some(min) = self.agents.values().filter_map(|c| c.max_relay_turns).min() {
            self.max_relay_turns = self.max_relay_turns.min(min);
        }
        for cmd in self.agents.values_mut() {
            cmd.engagement = None;
            cmd.accept_from = None;
            cmd.max_relay_turns = None;
        }
    }
}

fn default_ambient_responders() -> usize {
    1
}

fn default_max_concurrent_runs() -> usize {
    4
}

impl Default for AgentConfig {
    /// Matches serde defaults so empty files and load failures resolve alike.
    fn default() -> Self {
        Self {
            ambient_responders: 1,
            ambient_rotate_count: false,
            max_concurrent_runs: default_max_concurrent_runs(),
            reaction: Reaction::default(),
            engagement_window_secs: DEFAULT_ENGAGEMENT_WINDOW_SECS,
            ambient: Vec::new(),
            max_relay_turns: crate::agent::relay::DEFAULT_MAX_RELAY_TURNS,
            circles: BTreeMap::new(),
            agents: BTreeMap::new(),
        }
    }
}

impl AgentConfig {
    pub fn from_toml(text: &str) -> anyhow::Result<Self> {
        let mut cfg: Self = toml::from_str(text)?;
        cfg.migrate_legacy_agent_settings();
        Ok(cfg)
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
    fn scoped_settings_round_trip_through_toml() {
        let text = r#"
reaction = "push"
engagement_window_secs = 180
ambient = ["claude"]
max_relay_turns = 20

[agents.claude]
driver = "acp"
command = ["claude-agent-acp"]

[circles.work]
engagement_window_secs = 0
ambient = []
"#;
        let cfg = AgentConfig::from_toml(text).unwrap();
        let back = AgentConfig::from_toml(&toml::to_string_pretty(&cfg).unwrap()).unwrap();
        assert_eq!(back.resolved("work"), cfg.resolved("work"));
        assert_eq!(back.resolved("other"), cfg.resolved("other"));
    }

    #[test]
    fn a_circle_overrides_only_what_it_names() {
        let text = r#"
reaction = "push"
engagement_window_secs = 180
ambient = ["claude"]
max_relay_turns = 20

[agents.claude]
driver = "acp"
command = ["claude-agent-acp"]

[circles.social]
engagement_window_secs = 0
ambient = []
"#;
        let cfg = AgentConfig::from_toml(text).unwrap();

        let social = cfg.resolved("social");
        assert_eq!(social.engagement_window_secs, 0, "overridden");
        assert!(social.ambient.is_empty(), "overridden");
        assert_eq!(social.reaction, Reaction::Push, "inherited");
        assert_eq!(social.max_relay_turns, 20, "inherited");

        // A Circle that says nothing gets the global answers unchanged.
        let other = cfg.resolved("anything-else");
        assert_eq!(other.engagement_window_secs, 180);
        assert_eq!(other.ambient, vec!["claude"]);
    }

    #[test]
    fn an_empty_ambient_list_is_an_override_not_an_absence() {
        // `ambient = []` must mean "nobody reads the room here", not "inherit".
        // Collapsing the two would silently re-enable it.
        let text = r#"
ambient = ["claude"]
[agents.claude]
command = ["x"]
[circles.quiet]
ambient = []
"#;
        let cfg = AgentConfig::from_toml(text).unwrap();
        assert!(cfg.resolved("quiet").ambient.is_empty());
        assert_eq!(cfg.resolved("loud").ambient, vec!["claude"]);
    }

    #[test]
    fn legacy_per_agent_settings_are_migrated_into_the_global_scope() {
        // Configs written before settings were scoped must keep behaving the
        // same way, without the user editing anything.
        let text = r#"
reaction = "push"

[agents.claude]
driver = "acp"
command = ["claude-agent-acp"]
engagement = "ambient"
accept_from = "agents"
max_relay_turns = 6

[agents.codex]
driver = "acp"
command = ["codex-acp"]
"#;
        let cfg = AgentConfig::from_toml(text).unwrap();
        let global = cfg.resolved("");
        assert_eq!(global.ambient, vec!["claude"], "ambient carried over");
        assert_eq!(global.max_relay_turns, 6, "the tighter cap carried over");

        // The legacy keys are not written back, so saving completes the move.
        let saved = toml::to_string_pretty(&cfg).unwrap();
        assert!(!saved.contains("accept_from"), "dropped: no longer opt-in");
        assert!(!saved.contains("engagement ="), "moved to the scope");
        assert!(saved.contains("ambient"));
    }

    #[test]
    fn migration_never_raises_a_cap_somebody_lowered() {
        let text = r#"
[agents.a]
command = ["x"]
max_relay_turns = 4
[agents.b]
command = ["y"]
max_relay_turns = 30
"#;
        let cfg = AgentConfig::from_toml(text).unwrap();
        assert_eq!(cfg.resolved("").max_relay_turns, 4);
    }

    #[test]
    fn an_agent_without_engagement_keys_gets_the_safe_defaults() {
        let text = r#"
reaction = "push"

[agents.claude]
driver = "acp"
command = ["claude-agent-acp"]
"#;
        let cfg = AgentConfig::from_toml(text).unwrap();
        let global = cfg.resolved("");
        assert!(global.ambient.is_empty(), "reading the room stays opt-in");
        assert_eq!(
            global.max_relay_turns,
            crate::agent::relay::DEFAULT_MAX_RELAY_TURNS
        );
        assert_eq!(
            global.engagement_window_secs,
            DEFAULT_ENGAGEMENT_WINDOW_SECS
        );
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
    #[test]
    fn listener_limits_default_conservatively_and_inherit_by_circle() {
        let cfg = AgentConfig::from_toml("ambient_responders = 3\nambient_rotate_count = true\n[circles.quiet]\nambient_responders = 1\nambient_rotate_count = false\n").unwrap();
        assert_eq!(cfg.resolved("other").ambient_responders, 3);
        assert!(cfg.resolved("other").ambient_rotate_count);
        assert_eq!(cfg.resolved("quiet").ambient_responders, 1);
        assert!(!cfg.resolved("quiet").ambient_rotate_count);
        let defaults = AgentConfig::from_toml("").unwrap();
        assert_eq!(defaults.resolved("any").ambient_responders, 1);
        assert!(!defaults.resolved("any").ambient_rotate_count);
    }
}
