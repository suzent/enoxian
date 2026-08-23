use crate::control::{
    ChatActivity, ChatActivityKind, ChatMessage, CircleEvent, CHAT_ACTIVITY_KEY, CHAT_KEY,
};
use crate::daemon::DaemonState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    Json,
};
use serde::Deserialize;
use serde_json::json;
use tokio_stream::{wrappers::BroadcastStream, StreamExt};
use yrs::{Any, Array, Map, Out, ReadTxn, Transact, WriteTxn};

const TYPING_TTL_SECS: i64 = 6;
pub(crate) const AGENT_ACTIVITY_TTL_SECS: i64 = 45;

#[derive(Deserialize)]
pub struct ChatQuery {
    pub since: Option<i64>,
}

pub async fn get_chat(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Query(q): Query<ChatQuery>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "circle not found"})),
            )
                .into_response()
        }
    };
    let txn = match state.control.try_transact() {
        Ok(txn) => txn,
        Err(_) => return super::circle_busy(),
    };
    let Some(arr) = txn.get_array(CHAT_KEY) else {
        return Json(Vec::<ChatMessage>::new()).into_response();
    };
    let mut seen = std::collections::HashSet::new();
    let messages: Vec<ChatMessage> = arr
        .iter(&txn)
        .filter_map(|item| {
            if let Out::Any(Any::String(s)) = item {
                serde_json::from_str::<ChatMessage>(&s).ok()
            } else {
                None
            }
        })
        .filter(|message| seen.insert(message.id.clone()))
        .filter(|m| q.since.map(|s| m.ts > s).unwrap_or(true))
        .collect();
    Json(messages).into_response()
}

#[derive(Deserialize)]
pub struct PostChatRequest {
    pub text: String,
    pub agent_id: Option<String>,
}

pub async fn post_chat(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Json(req): Json<PostChatRequest>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "circle not found"})),
            )
                .into_response()
        }
    };

    let sender = req.agent_id.unwrap_or_else(|| "unknown".to_string());
    // A user/UI post fires mention triggers.
    match post_message(&state, sender, req.text, true) {
        Ok(id) => (StatusCode::CREATED, Json(json!({ "id": id }))).into_response(),
        Err(error) if error.to_string().contains("state busy") => super::circle_busy(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": error.to_string()})),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct PostActivityRequest {
    pub actor_id: String,
    pub typing: bool,
}

/// Return only live activity. The CRDT map may retain an expired value per
/// producer so that a disconnected peer cannot leave a permanent indicator.
pub async fn get_activity(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
) -> impl IntoResponse {
    let Some(state) = daemon.get(&circle_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "circle not found"})),
        )
            .into_response();
    };
    match live_activities(&state, chrono::Utc::now().timestamp()) {
        Some(activity) => Json(activity).into_response(),
        None => super::circle_busy(),
    }
}

/// Browser-originated activity is deliberately limited to `typing`; agent
/// lifecycle states are written internally only after a mention is accepted.
pub async fn post_activity(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Json(req): Json<PostActivityRequest>,
) -> impl IntoResponse {
    let Some(state) = daemon.get(&circle_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "circle not found"})),
        )
            .into_response();
    };
    let actor_id = req.actor_id.trim();
    if actor_id.is_empty() || actor_id.len() > 200 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid actor_id"})),
        )
            .into_response();
    }

    let now = chrono::Utc::now().timestamp();
    let expires_at = if req.typing {
        now + TYPING_TTL_SECS
    } else {
        now - 1
    };
    let activity = ChatActivity {
        activity_id: format!("typing:{actor_id}"),
        actor_id: actor_id.to_string(),
        peer_id: state.peer_id.clone(),
        kind: ChatActivityKind::Typing,
        message_id: None,
        updated_at: now,
        expires_at,
    };
    match put_activity(&state, activity) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(error) if error.to_string().contains("state busy") => super::circle_busy(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": error.to_string()})),
        )
            .into_response(),
    }
}

pub(crate) fn put_activity(
    state: &crate::state::AppState,
    activity: ChatActivity,
) -> anyhow::Result<()> {
    let raw = serde_json::to_string(&activity)?;
    let mut txn = state
        .control
        .try_transact_mut()
        .map_err(|_| anyhow::anyhow!("circle state busy"))?;
    let map = txn.get_or_insert_map(CHAT_ACTIVITY_KEY);
    let expired = map
        .iter(&txn)
        .filter_map(|(key, value)| match value {
            Out::Any(Any::String(raw)) => serde_json::from_str::<ChatActivity>(&raw)
                .ok()
                .filter(|item| !activity_is_live(item, chrono::Utc::now().timestamp()))
                .map(|_| key.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    for key in expired {
        map.remove(&mut txn, key.as_str());
    }
    map.insert(
        &mut txn,
        activity.activity_id.as_str(),
        Any::String(raw.as_str().into()),
    );
    drop(txn);
    let _ = state
        .events
        .send(CircleEvent::ChatActivityChanged { activity });
    Ok(())
}

fn live_activities(state: &crate::state::AppState, now: i64) -> Option<Vec<ChatActivity>> {
    let txn = state.control.try_transact().ok()?;
    let Some(map) = txn.get_map(CHAT_ACTIVITY_KEY) else {
        return Some(Vec::new());
    };
    Some(
        map.iter(&txn)
            .filter_map(|(_, value)| match value {
                Out::Any(Any::String(raw)) => serde_json::from_str::<ChatActivity>(&raw).ok(),
                _ => None,
            })
            .filter(|activity| activity_is_live(activity, now))
            .collect(),
    )
}

fn activity_is_live(activity: &ChatActivity, now: i64) -> bool {
    activity.expires_at > now
}

/// Post a chat message into the circle's control CRDT.
///
/// `fire_mentions` controls whether an `AgentMentioned` trigger event is emitted
/// for each mention in the text. User/UI posts pass `true` (a mention should
/// wake an agent). **Agent replies pass `false`** — otherwise an agent that
/// mentions another agent (or itself) in its reply sets off an endless
/// trigger loop. Mentions are always *stored* on the message (for chip
/// rendering) regardless; only the trigger side effect is gated.
///
/// Returns the new message id.
pub fn post_message(
    state: &crate::state::AppState,
    sender: String,
    text: String,
    fire_mentions: bool,
) -> anyhow::Result<String> {
    let mentions = crate::agent::mention::extract(&text);
    let msg = ChatMessage {
        id: uuid::Uuid::new_v4().to_string(),
        agent_id: sender,
        text,
        mentions: mentions.clone(),
        ts: chrono::Utc::now().timestamp(),
    };

    let json_str = serde_json::to_string(&msg)?;
    {
        let mut txn = state
            .control
            .try_transact_mut()
            .map_err(|_| anyhow::anyhow!("circle state busy"))?;
        let arr = txn.get_or_insert_array(CHAT_KEY);
        arr.push_back(&mut txn, Any::String(json_str.as_str().into()));
    }

    let _ = state.events.send(CircleEvent::MessagePosted {
        message: msg.clone(),
    });
    if fire_mentions {
        for mentioned in &mentions {
            let _ = state.events.send(CircleEvent::AgentMentioned {
                agent_id: mentioned.clone(),
                message: msg.clone(),
            });
        }
    }

    Ok(msg.id)
}

pub async fn chat_stream(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "circle not found"})),
            )
                .into_response()
        }
    };
    let rx = state.events.subscribe();
    let shutdown = daemon.shutdown_token.clone();
    let stream = BroadcastStream::new(rx).filter_map(|result| {
        result.ok().and_then(|ev| {
            // Chat messages/mentions drive the transcript; roster events let the
            // mention picker re-fetch members/presence live (e.g. when an agent
            // is added to a device's config). The frontend ignores any type it
            // doesn't handle, so forwarding these is safe.
            matches!(
                ev,
                CircleEvent::MessagePosted { .. }
                    | CircleEvent::AgentMentioned { .. }
                    | CircleEvent::ChatActivityChanged { .. }
                    | CircleEvent::MemberAdded { .. }
                    | CircleEvent::MemberRemoved { .. }
                    | CircleEvent::PresenceChanged { .. }
            )
            .then(|| serde_json::to_string(&ev).ok())
            .flatten()
            .map(|data| Ok::<_, std::convert::Infallible>(Event::default().data(data)))
        })
    });
    let stream = futures::StreamExt::take_until(stream, shutdown.cancelled_owned());
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn activity(expires_at: i64) -> ChatActivity {
        ChatActivity {
            activity_id: "typing:alice".to_string(),
            actor_id: "alice".to_string(),
            peer_id: "peer-alice".to_string(),
            kind: ChatActivityKind::Typing,
            message_id: None,
            updated_at: 10,
            expires_at,
        }
    }

    #[test]
    fn activity_expires_at_lease_boundary() {
        assert!(activity_is_live(&activity(11), 10));
        assert!(!activity_is_live(&activity(10), 10));
        assert!(!activity_is_live(&activity(9), 10));
    }

    #[test]
    fn activity_event_has_stable_wire_shape() {
        let event = CircleEvent::ChatActivityChanged {
            activity: activity(16),
        };
        let value = serde_json::to_value(event).unwrap();
        assert_eq!(value["type"], "chat_activity_changed");
        assert_eq!(value["activity"]["kind"], "typing");
        assert_eq!(value["activity"]["actor_id"], "alice");
    }
}
