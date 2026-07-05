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

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::agent::config::{AgentCommand, AgentConfig, Driver, Reaction};
use crate::agent::probe;
use crate::daemon::DaemonState;

#[derive(Serialize)]
struct AgentSummary {
    name: String,
    driver: String,
    /// The launch command. Shown so the operator can see exactly what a mention
    /// would run — this is launcher config, not a secret.
    command: Vec<String>,
    working_dir: Option<String>,
    /// Whether `command[0]` currently resolves on this machine's PATH. A
    /// configured-but-missing agent would fail at launch; the UI badges it.
    installed: bool,
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
            installed: cmd
                .command
                .first()
                .map(|p| probe::is_installed(p))
                .unwrap_or(false),
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

// ── Discovery ──────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct DiscoveredAgent {
    /// Suggested `@handle`.
    name: String,
    driver: String,
    command: Vec<String>,
    about: String,
    /// Whether the candidate's program resolves on this machine right now.
    installed: bool,
    /// Whether an agent by this name is already in the config (so the UI can
    /// show "added" instead of an add button).
    configured: bool,
}

/// List well-known agent candidates with their local install status.
///
/// Read-only probe: it checks each catalog program against PATH but never runs
/// anything. The frontend uses this to offer one-click adds for agents that are
/// actually installed and to mark ones already configured.
pub async fn discover_agents() -> impl IntoResponse {
    let cfg = AgentConfig::load();
    let discovered: Vec<DiscoveredAgent> = probe::CATALOG
        .iter()
        .map(|c| DiscoveredAgent {
            name: c.name.to_string(),
            driver: c.driver.to_string(),
            command: c.command.iter().map(|s| s.to_string()).collect(),
            about: c.about.to_string(),
            installed: probe::is_installed(c.program()),
            configured: cfg.resolve(c.name).is_some(),
        })
        .collect();
    Json(json!({ "agents": discovered }))
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

pub async fn add_agent(
    State(daemon): State<DaemonState>,
    Json(req): Json<AddAgentRequest>,
) -> impl IntoResponse {
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
    let resp = edit(move |cfg| {
        cfg.set_agent(&name, AgentCommand {
            command: req.command.clone(),
            driver,
            working_dir: req.working_dir.clone(),
        });
        Ok(())
    });
    readvertise_if_ok(&resp, &daemon);
    resp
}

#[derive(Deserialize)]
pub struct RemoveAgentRequest {
    pub name: String,
}

pub async fn remove_agent(
    State(daemon): State<DaemonState>,
    Json(req): Json<RemoveAgentRequest>,
) -> impl IntoResponse {
    let resp = edit(move |cfg| {
        if cfg.remove_agent(&req.name) {
            Ok(())
        } else {
            Err(format!("no agent named '{}'", req.name))
        }
    });
    readvertise_if_ok(&resp, &daemon);
    resp
}

/// After an agent add/remove succeeds, push the updated advertised list into
/// every active circle so peers' mention pickers reflect the change without a
/// daemon restart. Only fires on a 2xx edit — a failed/clobber-guarded edit
/// left the config untouched, so there's nothing new to advertise.
fn readvertise_if_ok(resp: &axum::response::Response, daemon: &DaemonState) {
    if resp.status().is_success() {
        crate::lifecycle::readvertise_local_agents(daemon);
    }
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
