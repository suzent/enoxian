use crate::control::{Presence, PRESENCE_KEY};
use crate::daemon::DaemonState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use serde_json::json;
use yrs::{Any, Map, MapRef, Out, Transact};

#[derive(Serialize)]
struct PresenceView {
    #[serde(flatten)]
    presence: Presence,
    connections: Vec<crate::state::PeerConnection>,
}

pub async fn get_who(
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
    let doc = &state.control;
    let presence_map: MapRef = doc.get_or_insert_map(PRESENCE_KEY);
    let txn = doc.transact();

    let mut result = Vec::new();
    for (_key, val) in presence_map.iter(&txn) {
        if let Out::Any(Any::String(s)) = val {
            if let Ok(p) = serde_json::from_str::<Presence>(&s) {
                let connections = state.peer_connections(&p.peer_id);
                result.push(PresenceView {
                    presence: p,
                    connections,
                });
            }
        }
    }
    result.sort_by(|a, b| a.presence.agent_id.cmp(&b.presence.agent_id));
    Json(result).into_response()
}
