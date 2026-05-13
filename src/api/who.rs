use axum::{extract::State, Json};
use yrs::{Any, Map, MapRef, Out, Transact};
use crate::state::AppState;
use crate::control::{Presence, PRESENCE_KEY};

pub async fn get_who(State(state): State<AppState>) -> Json<Vec<Presence>> {
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
    Json(result)
}
