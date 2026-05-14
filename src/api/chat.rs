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

    let mentions = parse_mentions(&req.text);
    let msg = ChatMessage {
        id: uuid::Uuid::new_v4().to_string(),
        agent_id: req.agent_id.unwrap_or_else(|| "unknown".to_string()),
        text: req.text,
        mentions: mentions.clone(),
        ts: chrono::Utc::now().timestamp(),
    };

    let json_str = match serde_json::to_string(&msg) {
        Ok(s) => s,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "serialize failed"}))).into_response(),
    };

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

    (StatusCode::CREATED, Json(json!({ "id": msg.id }))).into_response()
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

fn parse_mentions(text: &str) -> Vec<String> {
    let mut mentions = Vec::new();
    for word in text.split_whitespace() {
        if let Some(rest) = word.strip_prefix('@') {
            let mention: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            if !mention.is_empty() {
                mentions.push(mention);
            }
        }
    }
    mentions
}
