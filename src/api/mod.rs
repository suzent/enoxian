pub mod events;
pub mod lock;
pub mod status;
pub mod tasks;
pub mod who;

use axum::{Router, routing::{get, post}};
use crate::state::AppState;
use crate::sync_yjs::ws_handler::ws_yjs_handler;

pub fn router(state: AppState) -> Router {
    Router::new()
        // Yjs WebSocket
        .route("/ws/yjs", get(ws_yjs_handler))
        // REST API
        .route("/api/status",  get(status::get_status))
        .route("/api/who",     get(who::get_who))
        .route("/api/tasks",   get(tasks::get_tasks).post(tasks::create_task))
        .route("/api/claim",   post(lock::claim_task))
        .route("/api/done",    post(lock::done_task))
        .route("/api/bind",    post(lock::bind_path))
        .route("/api/release", post(lock::release_path))
        .route("/api/events",  get(events::sse_handler))
        .with_state(state)
        .layer(tower_http::cors::CorsLayer::permissive())
}
