//! Agent cards in a Circle's shared folder (see `agent::cards`).
//!
//! Lists every advertised agent with its card, for the settings panel. There
//! is no way to create one here: each device writes its own agents' cards (see
//! `agent::cards::ensure`).

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
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

fn not_found() -> axum::response::Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"error": "circle not found"})),
    )
        .into_response()
}
