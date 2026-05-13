use axum::{extract::State, Json};
use serde_json::{json, Value};
use crate::state::AppState;

pub async fn get_status(State(state): State<AppState>) -> Json<Value> {
    let doc_count = state.docs.len();
    Json(json!({
        "circle_id":   state.circle_id,
        "circle_name": state.circle_name,
        "sync_dir":    state.sync_dir.to_string_lossy(),
        "docs":        doc_count,
    }))
}
