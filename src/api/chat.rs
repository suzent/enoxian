use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{sse::{Event, KeepAlive, Sse}, IntoResponse},
    Json,
};
use serde::Deserialize;
use serde_json::json;
use tokio_stream::{wrappers::BroadcastStream, StreamExt};
use yrs::{Array, ArrayRef, Any, Out, Transact};
use crate::control::{ChatMessage, CircleEvent, CHAT_KEY};
use crate::daemon::DaemonState;

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
        None => return (StatusCode::NOT_FOUND, Json(json!({"error": "circle not found"}))).into_response(),
    };
    let arr: ArrayRef = state.control.get_or_insert_array(CHAT_KEY);
    let txn = state.control.transact();
    let messages: Vec<ChatMessage> = arr
        .iter(&txn)
        .filter_map(|item| {
            if let Out::Any(Any::String(s)) = item {
                serde_json::from_str::<ChatMessage>(&s).ok()
            } else {
                None
            }
        })
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
        None => return (StatusCode::NOT_FOUND, Json(json!({"error": "circle not found"}))).into_response(),
    };

    let sender = req.agent_id.unwrap_or_else(|| "unknown".to_string());
    match post_message(&state, sender, req.text) {
        Ok(id) => (StatusCode::CREATED, Json(json!({ "id": id }))).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "serialize failed"}))).into_response(),
    }
}

/// Post a chat message into the circle's control CRDT and fire the same events
/// a user post would. Reused by the HTTP handler and by the agent reaction loop
/// (so an agent's reply appears in the room and replicates to peers exactly like
/// any other message). Mentions in `text` are parsed and re-fired, so an agent
/// can address another agent — beware of loops when wiring auto-replies.
///
/// Returns the new message id.
pub fn post_message(
    state: &crate::state::AppState,
    sender: String,
    text: String,
) -> Result<String, serde_json::Error> {
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
        let arr: ArrayRef = state.control.get_or_insert_array(CHAT_KEY);
        let mut txn = state.control.transact_mut();
        arr.push_back(&mut txn, Any::String(json_str.as_str().into()));
    }

    let _ = state.events.send(CircleEvent::MessagePosted { message: msg.clone() });
    for mentioned in &mentions {
        let _ = state.events.send(CircleEvent::AgentMentioned {
            agent_id: mentioned.clone(),
            message: msg.clone(),
        });
    }

    Ok(msg.id)
}

pub async fn chat_stream(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => return (StatusCode::NOT_FOUND, Json(json!({"error": "circle not found"}))).into_response(),
    };
    let rx = state.events.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result| {
        result.ok().and_then(|ev| {
            matches!(ev, CircleEvent::MessagePosted { .. } | CircleEvent::AgentMentioned { .. })
                .then(|| serde_json::to_string(&ev).ok())
                .flatten()
                .map(|data| Ok::<_, std::convert::Infallible>(Event::default().data(data)))
        })
    });
    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

