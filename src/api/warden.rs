//! The warden's settings and records (see `agent::warden`).
//!
//! Device-local control plane, like `/api/agent-config`: the decider, its key
//! and the spec are this machine's, because this machine pays for them. The
//! key is write-only — it can be set, replaced or removed, never read back.
//! Per-Circle records (decisions, episodes) are this device's own and are not
//! synced.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::agent::config::AgentConfig;
use crate::agent::warden::{self, decider, log, policy, spec::WardenSpec};
use crate::daemon::DaemonState;

#[derive(Serialize)]
struct KindView {
    #[serde(flatten)]
    manifest: decider::Manifest,
    fields: std::collections::BTreeMap<&'static str, decider::Field>,
    /// "••••abcd" when a key is stored for this decider.
    key_hint: Option<String>,
}

fn kinds() -> Vec<KindView> {
    decider::catalog()
        .into_iter()
        .map(|manifest| KindView {
            fields: manifest.fields(),
            key_hint: decider::secrets::hint(&manifest.id),
            manifest,
        })
        .collect()
}

pub async fn get() -> impl IntoResponse {
    let cfg = AgentConfig::load();
    let (spec, source) = WardenSpec::load();
    let usable = decider::resolve(&cfg.warden).err();
    Json(json!({
        "mode": cfg.warden_mode,
        "config": cfg.warden,
        // Why the configured decider cannot be used, if it cannot: the rules
        // decide meanwhile, and the panel should say so rather than pretend.
        "problem": usable,
        "kinds": kinds(),
        "spec_toml": toml::to_string_pretty(&spec).unwrap_or_default(),
        "spec": spec,
        "spec_source": source,
        "spec_path": WardenSpec::path().ok().map(|p| p.display().to_string()),
    }))
}

#[derive(Deserialize)]
pub struct ConfigRequest {
    pub decider: Option<String>,
    /// `""` clears the override.
    pub endpoint: Option<String>,
    pub model: Option<String>,
    pub key_env: Option<String>,
    pub timeout_secs: Option<u64>,
    pub agent_messages: Option<bool>,
}

fn blank_to_none(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub async fn set_config(Json(req): Json<ConfigRequest>) -> axum::response::Response {
    edit(move |cfg| {
        if let Some(id) = req.decider {
            if decider::find(&id).is_none() {
                return Err(format!("no decider named {id:?}"));
            }
            cfg.warden.decider = id;
        }
        if let Some(endpoint) = req.endpoint {
            let endpoint = blank_to_none(endpoint);
            if let Some(e) = &endpoint {
                if !(e.starts_with("http://") || e.starts_with("https://")) {
                    return Err("the endpoint must be an http(s) URL".into());
                }
            }
            cfg.warden.endpoint = endpoint;
        }
        if let Some(model) = req.model {
            cfg.warden.model = blank_to_none(model);
        }
        if let Some(var) = req.key_env {
            cfg.warden.key_env = blank_to_none(var);
        }
        if let Some(secs) = req.timeout_secs {
            if !(1..=30).contains(&secs) {
                return Err("timeout_secs must be 1–30".into());
            }
            cfg.warden.timeout_secs = secs;
        }
        if let Some(on) = req.agent_messages {
            cfg.warden.agent_messages = on;
        }
        Ok(())
    })
}

#[derive(Deserialize)]
pub struct KeyRequest {
    pub decider: String,
    /// `null` or `""` removes the key.
    pub api_key: Option<String>,
}

pub async fn set_key(Json(req): Json<KeyRequest>) -> axum::response::Response {
    if decider::find(&req.decider).is_none() {
        return bad_request(format!("no decider named {:?}", req.decider));
    }
    match decider::secrets::set(&req.decider, req.api_key.as_deref()) {
        Ok(()) => Json(json!({ "key_hint": decider::secrets::hint(&req.decider) })).into_response(),
        Err(error) => server_error(error.to_string()),
    }
}

#[derive(Deserialize)]
pub struct SpecRequest {
    pub toml: String,
}

/// Replace the spec. Validated whole before anything is written.
pub async fn set_spec(Json(req): Json<SpecRequest>) -> axum::response::Response {
    let spec = match WardenSpec::from_toml(&req.toml) {
        Ok(spec) => spec,
        Err(error) => return bad_request(error),
    };
    match spec.save() {
        Ok(()) => Json(json!({ "spec": spec })).into_response(),
        Err(error) => server_error(error.to_string()),
    }
}

pub async fn reset_spec() -> axum::response::Response {
    match WardenSpec::reset() {
        Ok(()) => Json(json!({ "spec": WardenSpec::default() })).into_response(),
        Err(error) => server_error(error.to_string()),
    }
}

#[derive(Deserialize)]
pub struct TestRequest {
    pub text: String,
    #[serde(default)]
    pub circle_id: Option<String>,
    /// Ask as though an agent had posted it.
    #[serde(default)]
    pub from_agent: bool,
}

/// Ask the configured decider about a sample message, exactly as the warden
/// would, and show what it decided and what it cost. Runs nothing.
pub async fn test(
    State(daemon): State<DaemonState>,
    Json(req): Json<TestRequest>,
) -> axum::response::Response {
    if req.text.trim().is_empty() {
        return bad_request("text is required".into());
    }
    let cfg = AgentConfig::load();
    let (spec, _) = WardenSpec::load();
    let state = req.circle_id.as_deref().and_then(|id| daemon.get(id));
    let listeners: Vec<String> = match &state {
        Some(state) => cfg.resolved(&state.circle_id).ambient,
        None => cfg.ambient.clone(),
    };
    let listeners = if listeners.is_empty() {
        cfg.agents.keys().cloned().collect()
    } else {
        listeners
    };
    let ceiling = match &state {
        Some(state) => cfg.resolved(&state.circle_id).ambient_responders,
        None => cfg.ambient_responders,
    };
    let features = policy::Features {
        message_id: "test".into(),
        text: req.text.clone(),
        weighted_len: crate::agent::ambient::weighted_len(req.text.trim()),
        attachments: 0,
        context: Vec::new(),
        candidates: listeners
            .iter()
            .map(|name| policy::Candidate {
                name: name.clone(),
                summary: match &state {
                    Some(state) => crate::agent::cards::local_summary(state, name),
                    None => name.clone(),
                },
            })
            .collect(),
        ceiling,
        from_agent: req.from_agent,
    };
    let questions: Vec<String> = policy::questions(&spec, &features)
        .into_iter()
        .map(|q| q.name)
        .collect();
    let decision = warden::decide(&cfg.warden, &spec, &features).await;
    Json(json!({ "decision": decision, "questions": questions, "input": policy::input(&features) }))
        .into_response()
}

#[derive(Deserialize)]
pub struct RecentQuery {
    pub limit: Option<usize>,
    pub message_id: Option<String>,
}

/// Recent decisions in one Circle, newest first, for the activity panel.
pub async fn decisions(
    State(daemon): State<DaemonState>,
    Path(circle): Path<String>,
    Query(query): Query<RecentQuery>,
) -> axum::response::Response {
    let Some(state) = daemon.get(&circle) else {
        return not_found();
    };
    let limit = query.limit.unwrap_or(50).clamp(1, 500);
    let records: Vec<log::Record> = log::recent(&state.circle_dir, 5_000)
        .into_iter()
        .filter(|r| {
            query
                .message_id
                .as_ref()
                .is_none_or(|id| *id == r.message_id)
        })
        .take(limit)
        .map(|mut r| {
            // The panel needs the decision, not the message it already shows.
            if let Some(features) = r.features.as_mut() {
                features.context.clear();
                features.text = features.text.chars().take(200).collect();
            }
            r
        })
        .collect();
    let cfg = AgentConfig::load();
    Json(json!({
        "mode": cfg.resolved(&circle).warden_mode,
        "records": records,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct EpisodeQuery {
    /// Unix seconds; only messages first recorded at or after this.
    pub since: Option<i64>,
    /// `jsonl` for one episode per line, for experiment harnesses.
    pub format: Option<String>,
}

/// Every decision in one Circle joined with what became of it.
pub async fn episodes(
    State(daemon): State<DaemonState>,
    Path(circle): Path<String>,
    Query(query): Query<EpisodeQuery>,
) -> axum::response::Response {
    let Some(state) = daemon.get(&circle) else {
        return not_found();
    };
    let episodes = warden::episodes::build(&state, query.since);
    if query.format.as_deref() == Some("jsonl") {
        let mut body = String::new();
        for episode in &episodes {
            if let Ok(line) = serde_json::to_string(episode) {
                body.push_str(&line);
                body.push('\n');
            }
        }
        return (
            [(axum::http::header::CONTENT_TYPE, "application/x-ndjson")],
            body,
        )
            .into_response();
    }
    Json(json!({ "episodes": episodes })).into_response()
}

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
        Ok(()) => Json(json!({ "ok": true, "config": cfg.warden })).into_response(),
        Err(e) => server_error(e.to_string()),
    }
}

fn not_found() -> axum::response::Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"error": "circle not found"})),
    )
        .into_response()
}

fn bad_request(msg: String) -> axum::response::Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response()
}

fn server_error(msg: String) -> axum::response::Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": msg })),
    )
        .into_response()
}
