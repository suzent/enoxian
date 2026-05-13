pub mod events;
pub mod lock;
pub mod status;
pub mod tasks;
pub mod who;

use axum::{extract::State, routing::{get, post}, Json, Router};
use serde_json::json;
use crate::daemon::DaemonState;
use crate::sync_yjs::ws_handler::ws_yjs_handler;

async fn list_circles(State(daemon): State<DaemonState>) -> Json<serde_json::Value> {
    let circles: Vec<_> = daemon
        .list()
        .iter()
        .map(|s| json!({ "circle_id": s.circle_id, "circle_name": s.circle_name }))
        .collect();
    Json(json!(circles))
}

pub fn router(daemon: DaemonState) -> Router {
    Router::new()
        .route("/circles", get(list_circles))
        .route("/circles/{circle_id}/ws/yjs", get(ws_yjs_handler))
        .route("/circles/{circle_id}/api/status",  get(status::get_status))
        .route("/circles/{circle_id}/api/who",     get(who::get_who))
        .route("/circles/{circle_id}/api/tasks",   get(tasks::get_tasks).post(tasks::create_task))
        .route("/circles/{circle_id}/api/claim",   post(lock::claim_task))
        .route("/circles/{circle_id}/api/done",    post(lock::done_task))
        .route("/circles/{circle_id}/api/bind",    post(lock::bind_path))
        .route("/circles/{circle_id}/api/release", post(lock::release_path))
        .route("/circles/{circle_id}/api/events",  get(events::sse_handler))
        .with_state(daemon)
        .layer(tower_http::cors::CorsLayer::permissive())
}
