//! Device agent-config endpoint (read + local edit).
//!
//! Surfaces and edits `~/.enoxian/agents.toml` (the reaction policy and
//! configured agents). This is a **device-local control-plane** route served
//! over the loopback API, like `/api/identity` — it edits this machine's own
//! config, never synced state, so a remote peer cannot change it.
//!
//! The `push` reaction is the one sensitive setting (it lets a chat mention run
//! a local process). Editing agents is ordinary launcher config; the frontend
//! keeps a confirm step in front of switching to `push`, but the API itself
//! just applies what it is asked. See docs/plan/agent-workspaces.md →
//! Two-Layer Split.

use axum::{http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::agent::config::{AgentCommand, AgentConfig, Driver, Reaction};

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

// ── Editing ────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SetReactionRequest {
    /// "push" or "pull".
    pub reaction: String,
}

pub async fn set_reaction(Json(req): Json<SetReactionRequest>) -> impl IntoResponse {
    let reaction = match req.reaction.as_str() {
        "push" => Reaction::Push,
        "pull" => Reaction::Pull,
        other => return bad_request(format!("invalid reaction '{other}'")),
    };
    edit(|cfg| { cfg.reaction = reaction; Ok(()) })
}

#[derive(Deserialize)]
pub struct AddAgentRequest {
    pub name: String,
    /// "acp" (default) or "argv".
    #[serde(default)]
    pub driver: Option<String>,
    pub command: Vec<String>,
    #[serde(default)]
    pub working_dir: Option<String>,
}

pub async fn add_agent(Json(req): Json<AddAgentRequest>) -> impl IntoResponse {
    let driver = match req.driver.as_deref().unwrap_or("acp") {
        "acp" => Driver::Acp,
        "argv" => Driver::Argv,
        other => return bad_request(format!("invalid driver '{other}'")),
    };
    if req.name.trim().is_empty() {
        return bad_request("agent name is required".into());
    }
    if req.command.is_empty() {
        return bad_request("command is required".into());
    }
    let name = req.name.clone();
    edit(move |cfg| {
        cfg.set_agent(&name, AgentCommand {
            command: req.command.clone(),
            driver,
            working_dir: req.working_dir.clone(),
        });
        Ok(())
    })
}

#[derive(Deserialize)]
pub struct RemoveAgentRequest {
    pub name: String,
}

pub async fn remove_agent(Json(req): Json<RemoveAgentRequest>) -> impl IntoResponse {
    edit(move |cfg| {
        if cfg.remove_agent(&req.name) {
            Ok(())
        } else {
            Err(format!("no agent named '{}'", req.name))
        }
    })
}

/// Load-for-edit, apply a mutation, save. Refuses to touch an unparseable file
/// so a hand-edit in progress is never clobbered.
fn edit<F>(mutate: F) -> axum::response::Response
where
    F: FnOnce(&mut AgentConfig) -> Result<(), String>,
{
    let mut cfg = match AgentConfig::load_for_edit() {
        Ok(c) => c,
        Err(e) => return bad_request(e.to_string()),
    };
    if let Err(e) = mutate(&mut cfg) {
        return bad_request(e);
    }
    match cfg.save() {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

fn bad_request(msg: String) -> axum::response::Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response()
}
