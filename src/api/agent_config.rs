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
//! just applies what it is asked. See docs/concepts/proposals.md.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

use crate::agent::config::{AgentCommand, AgentConfig, Driver, Reaction};
use crate::agent::plugin;
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
    /// `ready`, `missing`, or `runtime_download`. The latter is deliberately
    /// not considered installed: a mention must not invoke a package manager.
    status: String,
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
    /// The answers that apply everywhere unless a Circle overrides them.
    global_settings: SettingsView,
    /// The active Circle's own answers: what it overrides, and what those
    /// settings actually resolve to there.
    circle: Option<CircleSettingsView>,
    agents: Vec<AgentSummary>,
}

#[derive(Serialize)]
struct AddressedAs {
    owner: String,
    device_label: String,
}

#[derive(Serialize)]
struct SettingsView {
    reaction: String,
    engagement_window_secs: i64,
    ambient: Vec<String>,
    max_relay_turns: u8,
}

#[derive(Serialize)]
struct CircleSettingsView {
    circle_id: String,
    /// How this Circle addresses this device: the `owner` and `device_label`
    /// from its roster entry.
    ///
    /// Not the same as the device-global handle. `owner` is fixed when you
    /// create or join a Circle and is part of your membership there, so
    /// changing the local handle does not rewrite it — and showing the local
    /// one as though it were the address would be a confidently wrong handle,
    /// which is the failure mode mentions are worst at reporting.
    addressed_as: Option<AddressedAs>,
    /// What this Circle sets for itself. A field absent here inherits.
    overrides: crate::agent::config::EngagementSettings,
    /// Global with the overrides applied — what actually happens here.
    effective: SettingsView,
}

fn settings_view(s: &crate::agent::config::ResolvedSettings) -> SettingsView {
    SettingsView {
        reaction: format!("{:?}", s.reaction).to_lowercase(),
        engagement_window_secs: s.engagement_window_secs,
        ambient: s.ambient.clone(),
        max_relay_turns: s.max_relay_turns,
    }
}

#[derive(Deserialize)]
pub struct ConfigQuery {
    /// Circle to report settings for. Omitted, only the global scope is shown.
    #[serde(default)]
    pub circle_id: Option<String>,
}

pub async fn get_agent_config(
    State(daemon): State<DaemonState>,
    Query(q): Query<ConfigQuery>,
) -> impl IntoResponse {
    let cfg = AgentConfig::load();
    let path = AgentConfig::path().ok();
    let configured = path.as_ref().map(|p| p.exists()).unwrap_or(false);

    let agents = cfg
        .agents
        .iter()
        .map(|(name, cmd)| AgentSummary {
            name: name.clone(),
            driver: format!("{:?}", cmd.driver).to_lowercase(),
            installed: plugin::command_status(&cmd.command) == "ready",
            status: plugin::command_status(&cmd.command).to_string(),
            command: cmd.command.clone(),
            working_dir: cmd.working_dir.clone(),
        })
        .collect();

    // The global scope is itself a resolution with nothing overriding it, so
    // both views are built the same way and cannot drift apart.
    let global = cfg.resolved("");
    let circle = q.circle_id.filter(|id: &String| !id.is_empty()).map(|id| {
        let effective = cfg.resolved(&id);
        // Resolved from the roster by peer id — the same source mention
        // targeting compares against, so the UI cannot disagree with it.
        let addressed_as = daemon.get(&id).and_then(|state| {
            state.self_member().and_then(|m| {
                (!m.owner.is_empty() && !m.device_label.is_empty()).then_some(AddressedAs {
                    owner: m.owner,
                    device_label: m.device_label,
                })
            })
        });
        CircleSettingsView {
            overrides: cfg.circles.get(&id).cloned().unwrap_or_default(),
            effective: settings_view(&effective),
            addressed_as,
            circle_id: id,
        }
    });

    Json(AgentConfigView {
        reaction: format!("{:?}", cfg.reaction).to_lowercase(),
        config_path: path
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        configured,
        global_settings: settings_view(&global),
        circle,
        agents,
    })
}

// ── Managed adapter plugins ───────────────────────────────────────────────

/// List built-in and third-party plugin manifests together with their actual
/// versioned install/configuration state.
pub async fn list_plugins() -> impl IntoResponse {
    Json(json!({ "plugins": plugin::views() }))
}

/// Explicitly install/repair one pinned plugin, then configure its agent handle
/// to use the managed executable. This is the networked control-plane step;
/// mention execution itself remains offline.
pub async fn install_plugin(
    State(daemon): State<DaemonState>,
    Path(plugin_id): Path<String>,
) -> axum::response::Response {
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        plugin::install(&plugin_id),
    )
    .await;
    match result {
        Ok(Ok(command)) => {
            crate::lifecycle::readvertise_local_agents(&daemon);
            Json(json!({
                "ok": true,
                "plugin": plugin_id,
                "command": command.command,
            }))
            .into_response()
        }
        Ok(Err(e)) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": format!("plugin install failed: {e:#}") })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::GATEWAY_TIMEOUT,
            Json(json!({ "error": "plugin install timed out after 5 minutes" })),
        )
            .into_response(),
    }
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
    edit(|cfg| {
        cfg.reaction = reaction;
        Ok(())
    })
}

/// Change engagement settings in one scope.
///
/// `circle_id` picks the scope: absent or empty means the global settings,
/// otherwise that Circle's overrides. Every setting is optional so the UI sends
/// only what changed, and at Circle scope an explicit `null` clears the
/// override so the setting inherits again.
///
/// Device-local, like everything in `agents.toml`. There is deliberately no way
/// for a remote member to reach this: the device that pays for an agent's
/// tokens is the device that decides what they are spent on.
#[derive(Deserialize)]
pub struct SetEngagementRequest {
    #[serde(default)]
    pub circle_id: Option<String>,
    /// "push" or "pull".
    #[serde(default, deserialize_with = "double_option")]
    pub reaction: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub engagement_window_secs: Option<Option<i64>>,
    /// Agents that read the room in this scope, by name.
    #[serde(default, deserialize_with = "double_option")]
    pub ambient: Option<Option<Vec<String>>>,
    #[serde(default, deserialize_with = "double_option")]
    pub max_relay_turns: Option<Option<u8>>,
}

/// Distinguish "field absent" from "field set to null".
///
/// At Circle scope they mean opposite things — leave this setting alone, versus
/// stop overriding it — and a plain `Option` collapses them into one.
fn double_option<'de, D, T>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Deserialize::deserialize(de).map(Some)
}

fn parse_reaction(value: &str) -> Result<Reaction, String> {
    match value {
        "push" => Ok(Reaction::Push),
        "pull" => Ok(Reaction::Pull),
        other => Err(format!("invalid reaction '{other}'")),
    }
}

fn validate(
    req: &SetEngagementRequest,
    known: &BTreeMap<String, AgentCommand>,
) -> Result<(), String> {
    if let Some(Some(window)) = req.engagement_window_secs {
        if window < 0 {
            return Err("engagement_window_secs cannot be negative".into());
        }
    }
    if let Some(Some(turns)) = req.max_relay_turns {
        if turns == 0 {
            return Err("max_relay_turns must be at least 1".into());
        }
    }
    if let Some(Some(value)) = &req.reaction {
        parse_reaction(value)?;
    }
    if let Some(Some(names)) = &req.ambient {
        // Naming an agent this device cannot run would look like it was
        // listening when nothing would ever wake it.
        for name in names {
            if !known.contains_key(name) {
                return Err(format!(
                    "no agent named '{name}' is configured on this device"
                ));
            }
        }
    }
    Ok(())
}

pub async fn set_engagement(
    State(daemon): State<DaemonState>,
    Json(req): Json<SetEngagementRequest>,
) -> impl IntoResponse {
    let scope = req.circle_id.clone().filter(|id| !id.is_empty());
    let resp = edit(move |cfg| {
        validate(&req, &cfg.agents)?;
        match scope {
            None => {
                // Global scope answers every question, so `null` is not a
                // meaningful value here — there is nothing above to inherit.
                if let Some(Some(value)) = &req.reaction {
                    cfg.reaction = parse_reaction(value)?;
                }
                if let Some(Some(window)) = req.engagement_window_secs {
                    cfg.engagement_window_secs = window;
                }
                if let Some(Some(names)) = req.ambient.clone() {
                    cfg.ambient = names;
                }
                if let Some(Some(turns)) = req.max_relay_turns {
                    cfg.max_relay_turns = turns;
                }
            }
            Some(circle_id) => {
                let over = cfg.circles.entry(circle_id.clone()).or_default();
                if let Some(value) = &req.reaction {
                    over.reaction = match value {
                        Some(v) => Some(parse_reaction(v)?),
                        None => None,
                    };
                }
                if let Some(value) = req.engagement_window_secs {
                    over.engagement_window_secs = value;
                }
                if let Some(value) = req.ambient.clone() {
                    over.ambient = value;
                }
                if let Some(value) = req.max_relay_turns {
                    over.max_relay_turns = value;
                }
                // An override that overrides nothing is noise in the file.
                if over == &crate::agent::config::EngagementSettings::default() {
                    cfg.circles.remove(&circle_id);
                }
            }
        }
        Ok(())
    });
    // `ambient_agents` is advertised in the roster so every peer can see who is
    // listening. Without this the switch would take effect locally while the
    // rest of the Circle kept seeing the old answer.
    readvertise_if_ok(&resp, &daemon);
    resp
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
        cfg.set_agent(
            &name,
            AgentCommand {
                command: req.command.clone(),
                driver,
                working_dir: req.working_dir.clone(),
                ..Default::default()
            }
            .inheriting_delegation(cfg.resolve(&name)),
        );
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
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

fn bad_request(msg: String) -> axum::response::Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response()
}
