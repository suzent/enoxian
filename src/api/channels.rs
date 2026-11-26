//! Topic channel endpoints. See `crate::channel` and
//! `docs/development/designs/channels.md`.

use crate::channel::{self, ChannelInfo};
use crate::control::CircleEvent;
use crate::daemon::DaemonState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use yrs::Transact;

/// A channel as the list endpoint returns it: its entry plus enough about its
/// transcript for a viewer to show unread state.
#[derive(Serialize)]
pub struct ChannelSummary {
    #[serde(flatten)]
    pub info: ChannelInfo,
    pub message_count: usize,
    pub last_message_id: Option<String>,
    pub last_ts: Option<i64>,
}

pub fn summaries(state: &crate::state::AppState) -> Vec<ChannelSummary> {
    let all = state.all_messages();
    channel::list_with_general(state)
        .into_iter()
        .map(|info| {
            let mut count = 0;
            let mut last = None;
            for m in all.iter().filter(|m| m.channel == info.id) {
                count += 1;
                last = Some(m);
            }
            ChannelSummary {
                message_count: count,
                last_message_id: last.map(|m| m.id.clone()),
                last_ts: last.map(|m| m.ts),
                info,
            }
        })
        .collect()
}

pub async fn list_channels(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
) -> impl IntoResponse {
    let Some(state) = daemon.get(&circle_id) else {
        return not_found("circle not found");
    };
    if state.control.try_transact().is_err() {
        return super::circle_busy();
    }
    Json(summaries(&state)).into_response()
}

#[derive(Deserialize)]
pub struct CreateChannelRequest {
    pub name: String,
    #[serde(default)]
    pub purpose: String,
    pub agent_id: Option<String>,
    pub actor_token: Option<String>,
}

pub async fn create_channel(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Json(req): Json<CreateChannelRequest>,
) -> impl IntoResponse {
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
    match channel::create(&state, &req.name, &req.purpose, &actor.agent_id) {
        Ok(info) => {
            let _ = state.events.send(CircleEvent::ChannelsChanged {});
            (StatusCode::CREATED, Json(info)).into_response()
        }
        Err(error) if error.contains("busy") => super::circle_busy(),
        Err(error) => (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response(),
    }
}

#[derive(Deserialize)]
pub struct UpdateChannelRequest {
    pub purpose: Option<String>,
    pub archived: Option<bool>,
}

pub async fn update_channel(
    State(daemon): State<DaemonState>,
    Path((circle_id, reference)): Path<(String, String)>,
    Json(req): Json<UpdateChannelRequest>,
) -> impl IntoResponse {
    let Some(state) = daemon.get(&circle_id) else {
        return not_found("circle not found");
    };
    let id = match channel::resolve(&state, Some(&reference)) {
        Ok(Some(id)) => id,
        Ok(None) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "#general cannot be changed"})),
            )
                .into_response()
        }
        Err(error) => return not_found(&error),
    };
    match channel::update(&state, &id, req.purpose.as_deref(), req.archived) {
        Ok(info) => {
            let _ = state.events.send(CircleEvent::ChannelsChanged {});
            Json(info).into_response()
        }
        Err(error) if error.contains("busy") => super::circle_busy(),
        Err(error) => (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response(),
    }
}

fn not_found(error: &str) -> axum::response::Response {
    (StatusCode::NOT_FOUND, Json(json!({ "error": error }))).into_response()
}
