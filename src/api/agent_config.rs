//! Read-only device agent-config endpoint.
//!
//! Surfaces `~/.enoxian/agents.toml` (the reaction policy and configured
//! agents) so the frontend can *show* how this device reacts to chat mentions.
//! It is intentionally read-only: the `push` reaction is the toggle that lets a
//! chat mention run a local process, so arming it stays a deliberate file edit,
//! not a UI click (see docs/plan/agent-workspaces.md → Two-Layer Split).
//!
//! This is a device-level route (not circle-scoped), like `/api/identity`.

use axum::{response::IntoResponse, Json};
use serde::Serialize;

use crate::agent::config::AgentConfig;

#[derive(Serialize)]
struct AgentSummary {
    name: String,
    driver: String,
    /// The launch command. Shown so the operator can see exactly what a mention
    /// would run — this is launcher config, not a secret.
    command: Vec<String>,
    working_dir: Option<String>,
}

#[derive(Serialize)]
struct AgentConfigView {
    /// "push" or "pull".
    reaction: String,
    /// Absolute path of the config file, so the UI can tell the user what to
    /// edit (editing stays file-only).
    config_path: String,
    /// True if the file actually exists (vs. defaulted-empty).
    configured: bool,
    agents: Vec<AgentSummary>,
}

pub async fn get_agent_config() -> impl IntoResponse {
    let cfg = AgentConfig::load();
    let path = AgentConfig::path().ok();
    let configured = path.as_ref().map(|p| p.exists()).unwrap_or(false);

    let agents = cfg
        .agents
        .iter()
        .map(|(name, cmd)| AgentSummary {
            name: name.clone(),
            driver: format!("{:?}", cmd.driver).to_lowercase(),
            command: cmd.command.clone(),
            working_dir: cmd.working_dir.clone(),
        })
        .collect();

    Json(AgentConfigView {
        reaction: format!("{:?}", cfg.reaction).to_lowercase(),
        config_path: path
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        configured,
        agents,
    })
}
