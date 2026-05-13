use axum::{extract::{Path, State}, http::StatusCode, response::IntoResponse, Json};
use serde_json::json;
use yrs::{Any, Map, MapRef, Out, Transact};
use crate::control::{Presence, PRESENCE_KEY};
use crate::daemon::DaemonState;

pub async fn get_who(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => return (StatusCode::NOT_FOUND, Json(json!({"error": "circle not found"}))).into_response(),
    };
    let doc = &state.control;
    let presence_map: MapRef = doc.get_or_insert_map(PRESENCE_KEY);
    let txn = doc.transact();

    let mut result = Vec::new();
    for (_key, val) in presence_map.iter(&txn) {
        if let Out::Any(Any::String(s)) = val {
            if let Ok(p) = serde_json::from_str::<Presence>(&s) {
                result.push(p);
            }
        }
    }
    Json(result).into_response()
}
