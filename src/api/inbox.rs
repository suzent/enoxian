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

use crate::agent::claims;
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
                "claimed_by": live.get(&m.id).and_then(|c| c.first()).map(claim_json),
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

    let mut all_claims: Vec<_> = live.values().flatten().map(claim_json).collect();
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

fn claim_json(claim: &claims::Claim) -> serde_json::Value {
    json!({
        "message_id": claim.message_id,
        "agent": claim.agent,
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
