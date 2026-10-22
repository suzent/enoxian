//! Agent cards in a Circle's shared folder (see `agent::cards`).
//!
//! Reading lists every advertised agent with its card, so the settings panel
//! can show which of this device's agents still have none. Creating writes the
//! template for one of *this device's* agents only: a card is a file in the
//! shared folder like any other, but nobody should be minting cards for agents
//! on someone else's machine.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::agent::cards;
use crate::daemon::DaemonState;

#[derive(Serialize)]
struct CardView {
    handle: String,
    owner: String,
    device: String,
    agent: String,
    path: String,
    /// True for an agent configured on this device.
    local: bool,
    exists: bool,
    summary: String,
}

pub async fn list(
    State(daemon): State<DaemonState>,
    Path(circle): Path<String>,
) -> axum::response::Response {
    let Some(state) = daemon.get(&circle) else {
        return not_found();
    };
    let cards: Vec<CardView> = cards::roster_cards(&state)
        .into_iter()
        .map(|(member, agent, card)| CardView {
            handle: format!("@{}/{}/{}", member.owner, member.device_label, agent),
            path: cards::relative_path(&member.owner, &member.device_label, &agent),
            local: member.peer_id == state.peer_id,
            exists: card.is_some(),
            summary: card.map(|c| c.summary).unwrap_or_default(),
            owner: member.owner,
            device: member.device_label,
            agent,
        })
        .collect();
    Json(json!({ "cards": cards })).into_response()
}

#[derive(Deserialize)]
pub struct CreateRequest {
    pub agent: String,
}

pub async fn create(
    State(daemon): State<DaemonState>,
    Path(circle): Path<String>,
    Json(req): Json<CreateRequest>,
) -> axum::response::Response {
    let Some(state) = daemon.get(&circle) else {
        return not_found();
    };
    if crate::agent::config::AgentConfig::load()
        .resolve(&req.agent)
        .is_none()
    {
        return bad_request(format!(
            "no agent named '{}' is configured on this device",
            req.agent
        ));
    }
    let Some(me) = state.self_member().filter(|m| !m.device_label.is_empty()) else {
        return bad_request(
            "this device has no label in the Circle yet, so its agents have no handle".into(),
        );
    };
    match cards::create(&state.workspace, &me.owner, &me.device_label, &req.agent) {
        Ok((path, created)) => Json(json!({ "path": path, "created": created })).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("could not write the card: {error}") })),
        )
            .into_response(),
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
