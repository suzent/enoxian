//! An agent's view of what is waiting for it, and the means to take one.
//!
//! The execution inbox *pushes*: the reaction loop decides who gets a message
//! and hands it to them. This is the other direction. An agent — or a person —
//! can ask what is waiting, see who is already on what, and take an unaddressed
//! message so the ambient listeners in the room leave it alone.
//!
//! Taking a message is claiming it: the claim is a replicated `Working`
//! activity, so every device's drain sees it and backs off (see
//! [`crate::agent::claims`]). It expires by itself, which is what stops a claim
//! nobody follows up on from silencing a message for good.
//!
//! One route goes the other way round: `inject` queues a private message for
//! one local agent without posting it anywhere (see [`crate::agent::private`]).

use crate::agent::claims;
use crate::agent::label::Roster;
use crate::control::{Author, ChatActivity, ChatActivityKind, ChatMessage};
use crate::daemon::DaemonState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;

/// How far back the open list looks, in messages.
///
/// A count rather than a time window: this is a view, but every other decision
/// in this area avoids comparing clocks across devices, and a count cannot be
/// skewed.
const OPEN_WINDOW: usize = 50;

#[derive(Default, Deserialize)]
pub struct InboxQuery {
    /// Whose inbox. Omitted, `waiting` covers every agent on this device.
    pub agent: Option<String>,
}

pub async fn get_inbox(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Query(query): Query<InboxQuery>,
) -> Response {
    let Some(state) = daemon.get(&circle_id) else {
        return not_found("circle not found");
    };
    let now = chrono::Utc::now().timestamp();
    let Some(activities) = crate::api::chat::live_activities(&state, now) else {
        return super::circle_busy();
    };
    let live = claims::live_claims(&activities);
    let roster = Roster::load(&state);
    let transcript = state.transcript();

    let answered: std::collections::HashSet<&str> = transcript
        .iter()
        .filter(|m| m.author == Author::Agent)
        .filter_map(|m| m.reply_to.as_deref())
        .collect();
    // The same test the room uses to decide what is worth a turn, so the two
    // cannot drift. Against real Circles the unfiltered list was mostly "hi",
    // "123" and connectivity checks, burying the one question actually waiting.
    let open: Vec<_> = transcript
        .iter()
        .filter(|m| !answered.contains(m.id.as_str()))
        .filter(|m| crate::agent::ambient::skip_reason(m, names_an_agent(m)).is_none())
        .rev()
        .take(OPEN_WINDOW)
        .map(|m| {
            json!({
                "message_id": m.id,
                "from": m.agent_id,
                "text": m.text,
                "ts": m.ts,
                "claimed_by": live.get(&m.id).and_then(|c| c.first()).map(|c| claim_json(c, &roster)),
            })
        })
        .collect();

    let waiting: Vec<_> = state
        .execution_inbox
        .read()
        .unwrap()
        .as_ref()
        .and_then(std::sync::Weak::upgrade)
        .map(|inbox| inbox.entries())
        .unwrap_or_default()
        .into_iter()
        .filter(|e| {
            matches!(
                e.status,
                crate::agent::inbox::Status::Pending | crate::agent::inbox::Status::Running
            )
        })
        .filter(|e| {
            query
                .agent
                .as_deref()
                .is_none_or(|a| a.eq_ignore_ascii_case(&e.request.agent))
        })
        .map(|e| {
            json!({
                "run_id": e.run_id,
                "agent": e.request.agent,
                "message_id": e.request.message.id,
                "text": e.request.message.text,
                "status": e.status,
                "ambient": e.request.ambient,
            })
        })
        .collect();

    let mut all_claims: Vec<_> = live
        .values()
        .flatten()
        .map(|c| claim_json(c, &roster))
        .collect();
    all_claims.sort_by(|a, b| a["message_id"].as_str().cmp(&b["message_id"].as_str()));

    Json(json!({
        "agent": query.agent,
        "waiting": waiting,
        "open": open,
        "claims": all_claims,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct ClaimRequest {
    /// A full message id, or an unambiguous prefix of one.
    pub message_id: String,
    pub agent_id: Option<String>,
    pub actor_token: Option<String>,
    /// How long to hold it. Clamped; see [`claims::MAX_CLAIM_SECS`].
    pub ttl_secs: Option<i64>,
}

pub async fn claim(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Json(req): Json<ClaimRequest>,
) -> Response {
    let ttl = req
        .ttl_secs
        .unwrap_or(claims::DEFAULT_CLAIM_SECS)
        .clamp(30, claims::MAX_CLAIM_SECS);
    set_claim(daemon, circle_id, req, Some(ttl))
}

pub async fn release(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Json(req): Json<ClaimRequest>,
) -> Response {
    set_claim(daemon, circle_id, req, None)
}

/// Take (`Some(ttl)`) or give back (`None`) a claim.
///
/// Only ever touches the caller's own `claim:` record. A running turn's
/// heartbeat lives under a different key, so releasing a claim can never
/// cancel a turn that happens to be working on the same message.
fn set_claim(
    daemon: DaemonState,
    circle_id: String,
    req: ClaimRequest,
    ttl: Option<i64>,
) -> Response {
    let Some(state) = daemon.get(&circle_id) else {
        return not_found("circle not found");
    };
    let actor = match super::actor::resolve_actor(
        &state,
        req.actor_token.as_deref(),
        req.agent_id,
        "unknown",
    ) {
        Ok(actor) => actor,
        Err(error) => return error.into_response(),
    };
    let message = match resolve_message(&state.transcript(), &req.message_id) {
        Ok(message) => message,
        Err(Lookup::TooShort) => return bad_request("message id must be at least 4 characters"),
        Err(Lookup::NotFound) => return not_found("no message with that id"),
        Err(Lookup::Ambiguous) => {
            return (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "that prefix matches more than one message; give more of the id"
                })),
            )
                .into_response()
        }
    };
    let now = chrono::Utc::now().timestamp();
    let expires_at = ttl.map_or(now - 1, |ttl| now + ttl);
    let activity = ChatActivity {
        activity_id: claims::claim_activity_id(&message.id, &actor.agent_id, &state.peer_id),
        actor_id: actor.agent_id.clone(),
        peer_id: state.peer_id.clone(),
        kind: ChatActivityKind::Working,
        detail: None,
        message_id: Some(message.id.clone()),
        updated_at: now,
        expires_at,
    };
    match crate::api::chat::put_activity(&state, activity) {
        Ok(()) => Json(json!({
            "message_id": message.id,
            "agent": actor.agent_id,
            "claimed": ttl.is_some(),
            "expires_at": ttl.map(|_| expires_at),
        }))
        .into_response(),
        Err(error) if error.to_string().contains("state busy") => super::circle_busy(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": error.to_string()})),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct InjectRequest {
    /// The agent on this device to tell.
    pub agent: String,
    pub text: String,
    /// Who the agent is told said it. Defaults to this device's member.
    pub from: Option<String>,
}

/// Tell one agent on this device something the rest of the Circle does not
/// hear (see [`crate::agent::private`]).
///
/// Nothing is written to the control doc. The turn is queued on this device's
/// execution inbox and runs like an addressed mention, except that its reply
/// is kept on the inbox entry instead of posted to the room.
pub async fn inject(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Json(req): Json<InjectRequest>,
) -> Response {
    let Some(state) = daemon.get(&circle_id) else {
        return not_found("circle not found");
    };
    let inbox = state
        .execution_inbox
        .read()
        .unwrap()
        .as_ref()
        .and_then(std::sync::Weak::upgrade);
    let Some(inbox) = inbox else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "execution owner unavailable; is this Circle running?"})),
        )
            .into_response();
    };
    match inject_into(
        &state,
        &inbox,
        &crate::agent::config::AgentConfig::load(),
        req,
    ) {
        Ok(body) => Json(body).into_response(),
        Err((status, error)) => (status, Json(json!({"error": error}))).into_response(),
    }
}

/// Validate and admit a private delivery.
pub(crate) fn inject_into(
    state: &crate::state::AppState,
    inbox: &crate::agent::inbox::Inbox,
    cfg: &crate::agent::config::AgentConfig,
    req: InjectRequest,
) -> Result<serde_json::Value, (StatusCode, String)> {
    use crate::agent::inbox::Admission;
    let agent = req.agent.trim();
    if req.text.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "nothing to say".into()));
    }
    if cfg.resolve(agent).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            // A config key, and "on this device" already says whose.
            format!(
                "no agent named `{}` is configured on this device",
                req.agent.trim()
            ),
        ));
    }
    // Refused rather than queued: under pull nothing would ever run it, and a
    // private delivery that silently never arrives is the worst outcome here.
    if cfg.resolved(&state.circle_id).reaction != crate::agent::config::Reaction::Push {
        return Err((
            StatusCode::CONFLICT,
            "this device is set to pull, so it launches no agent turns; switch it to push".into(),
        ));
    }
    let request = crate::agent::private::request(state, agent, &req.text, req.from.as_deref());
    let message_id = request.message.id.clone();
    let now = chrono::Utc::now().timestamp();
    match inbox.admit(request, cfg.resolved(&state.circle_id).max_relay_turns, now) {
        Ok(Admission::Accepted { entry, displaced }) => {
            for dropped in displaced {
                tracing::info!(
                    "[agent] private delivery displaced queued turn {}: {}",
                    dropped.run_id,
                    dropped.detail.as_deref().unwrap_or("queue full")
                );
            }
            state.execution_wake.notify_one();
            Ok(json!({
                "run_id": entry.run_id,
                "message_id": message_id,
                "agent": entry.request.agent,
                "status": entry.status,
            }))
        }
        Ok(Admission::Rejected(entry)) => Err((
            StatusCode::TOO_MANY_REQUESTS,
            entry.detail.unwrap_or_else(|| "not admitted".into()),
        )),
        Ok(Admission::Duplicate) => Err((StatusCode::CONFLICT, "already queued".into())),
        Err(error) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("execution inbox could not be saved: {error}"),
        )),
    }
}

/// Why a message id did not resolve.
enum Lookup {
    TooShort,
    NotFound,
    /// More than one message starts with it. Refused rather than guessed: a
    /// claim silences the room for that message, so claiming the wrong one is
    /// not a harmless mistake.
    Ambiguous,
}

/// Find a message by id or by an unambiguous prefix of one.
///
/// Prefixes because these ids are UUIDs and the CLI shows eight characters;
/// asking a person to paste thirty-six to claim something is asking them not to.
fn resolve_message(transcript: &[ChatMessage], id: &str) -> Result<ChatMessage, Lookup> {
    let id = id.trim();
    if id.len() < 4 {
        return Err(Lookup::TooShort);
    }
    if let Some(exact) = transcript.iter().find(|m| m.id == id) {
        return Ok(exact.clone());
    }
    let matches: Vec<&ChatMessage> = transcript.iter().filter(|m| m.id.starts_with(id)).collect();
    match matches.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(Lookup::NotFound),
        _ => Err(Lookup::Ambiguous),
    }
}

fn names_an_agent(message: &ChatMessage) -> bool {
    message.mentions.iter().any(|m| {
        crate::agent::mention::Mention::parse(m)
            .and_then(|parsed| parsed.agent_target().map(|_| ()))
            .is_some()
    })
}

/// A claim for the API. `agent` and `peer_id` stay raw, as identifiers;
/// `label` is the same agent named for a person to read, with its machine —
/// otherwise a claim by one device's `claude` reads the same as another's.
fn claim_json(claim: &claims::Claim, roster: &Roster) -> serde_json::Value {
    json!({
        "message_id": claim.message_id,
        "agent": claim.agent,
        "label": roster.agent(&claim.agent, &claim.peer_id),
        "peer_id": claim.peer_id,
        "expires_at": claim.expires_at,
        "explicit": claim.explicit,
    })
}

fn not_found(error: &str) -> Response {
    (StatusCode::NOT_FOUND, Json(json!({"error": error}))).into_response()
}

fn bad_request(error: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": error}))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::config::{AgentCommand, AgentConfig, Reaction};
    use crate::agent::inbox::{Inbox, Status};

    fn state(dir: &std::path::Path) -> crate::state::AppState {
        crate::state::AppState::new(
            "c1".into(),
            "circle".into(),
            dir.to_path_buf(),
            dir.to_path_buf(),
            String::new(),
            "suzy".into(),
            1,
            "local".into(),
            crate::config::JoinPolicy::Manual,
            "owner".into(),
            crate::mls::new_mls_state(crate::mls::MlsIdentity::generate("local").unwrap(), None),
        )
    }

    fn push() -> AgentConfig {
        let mut cfg = AgentConfig {
            reaction: Reaction::Push,
            ..Default::default()
        };
        cfg.agents.insert("claude".into(), AgentCommand::default());
        cfg
    }

    fn ask(agent: &str, text: &str) -> InjectRequest {
        InjectRequest {
            agent: agent.into(),
            text: text.into(),
            from: None,
        }
    }

    #[test]
    fn inject_queues_a_private_turn_and_posts_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let state = state(dir.path());
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let body = inject_into(&state, &inbox, &push(), ask("claude", "cap is 25%")).unwrap();

        let entry = &inbox.entries()[0];
        assert_eq!(body["run_id"], entry.run_id.as_str());
        assert_eq!(entry.status, Status::Pending);
        assert_eq!(entry.request.agent, "claude");
        assert_eq!(entry.request.task, "cap is 25%");
        assert_eq!(
            entry.request.message.agent_id, "suzy",
            "said by this device's member"
        );
        assert!(!entry.request.ambient);
        assert!(crate::agent::private::is_private(&entry.request.message.id));
        assert_eq!(body["message_id"], entry.request.message.id.as_str());
        assert!(state.transcript().is_empty());
    }

    #[test]
    fn inject_refuses_what_would_never_run() {
        let dir = tempfile::tempdir().unwrap();
        let state = state(dir.path());
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let status = |cfg: &AgentConfig, req| inject_into(&state, &inbox, cfg, req).unwrap_err().0;

        assert_eq!(status(&push(), ask("codex", "hi")), StatusCode::NOT_FOUND);
        assert_eq!(
            status(&push(), ask("claude", "  ")),
            StatusCode::BAD_REQUEST
        );
        let mut pull = push();
        pull.reaction = Reaction::Pull;
        assert_eq!(status(&pull, ask("claude", "hi")), StatusCode::CONFLICT);
        assert!(inbox.entries().is_empty());
    }
}
