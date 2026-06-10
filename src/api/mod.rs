pub mod chat;
pub mod events;
pub mod files;
pub mod identity;
pub mod lifecycle;
pub mod lock;
pub mod management;
pub mod members;
pub mod proposals;
pub mod shutdown;
pub mod status;
pub mod tasks;
pub mod who;

use crate::daemon::DaemonState;
use crate::sync_yjs::ws_handler::ws_yjs_handler;
use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use serde_json::json;

async fn list_circles(State(_daemon): State<DaemonState>) -> Json<serde_json::Value> {
    let configs = crate::config::load_all().unwrap_or_default();
    let circles: Vec<_> = configs
        .into_iter()
        .map(|c| json!({ "circle_id": c.circle_id, "circle_name": c.circle_name, "disabled": c.disabled }))
        .collect();
    Json(json!(circles))
}

pub fn router(daemon: DaemonState) -> Router {
    Router::new()
        .route("/shutdown", post(shutdown::shutdown))
        .route("/circles", get(list_circles))
        .route("/circles/{circle_id}/ws/yjs", get(ws_yjs_handler))
        .route("/circles/{circle_id}/api/status", get(status::get_status))
        .route("/circles/{circle_id}/api/who", get(who::get_who))
        .route(
            "/circles/{circle_id}/api/tasks",
            get(tasks::get_tasks).post(tasks::create_task),
        )
        .route("/circles/{circle_id}/api/claim", post(lock::claim_task))
        .route("/circles/{circle_id}/api/done", post(lock::done_task))
        .route("/circles/{circle_id}/api/bind", post(lock::bind_path))
        .route("/circles/{circle_id}/api/release", post(lock::release_path))
        .route("/circles/{circle_id}/api/events", get(events::sse_handler))
        .route("/circles/{circle_id}/api/files", get(files::list_files))
        .route(
            "/circles/{circle_id}/api/files/create",
            post(files::create_file),
        )
        .route(
            "/circles/{circle_id}/api/files/rename",
            post(files::rename_file),
        )
        .route(
            "/circles/{circle_id}/api/files/delete",
            post(files::delete_file),
        )
        // M14 proposals
        .route(
            "/circles/{circle_id}/api/proposals",
            get(proposals::list_proposals),
        )
        .route(
            "/circles/{circle_id}/api/proposals/{proposal_id}",
            get(proposals::get_proposal),
        )
        .route(
            "/circles/{circle_id}/api/proposals/{proposal_id}/accept",
            post(proposals::accept_proposal),
        )
        .route(
            "/circles/{circle_id}/api/proposals/{proposal_id}/reject",
            post(proposals::reject_proposal),
        )
        .route(
            "/circles/{circle_id}/api/proposals/{proposal_id}/revert",
            post(proposals::revert_proposal),
        )
        // M9 chat
        .route(
            "/circles/{circle_id}/api/chat",
            get(chat::get_chat).post(chat::post_chat),
        )
        .route(
            "/circles/{circle_id}/api/chat/stream",
            get(chat::chat_stream),
        )
        // M4 lifecycle
        .route("/circles/{circle_id}/stop", post(lifecycle::stop_circle))
        .route("/circles/{circle_id}/start", post(lifecycle::start_circle))
        // M6 members
        .route(
            "/circles/{circle_id}/members",
            get(members::list_members).post(members::add_member),
        )
        .route(
            "/circles/{circle_id}/members/remove",
            post(members::remove_member),
        )
        .route(
            "/circles/{circle_id}/members/promote",
            post(members::promote_member),
        )
        .route(
            "/circles/{circle_id}/members/pending",
            get(members::list_pending),
        )
        .route(
            "/circles/{circle_id}/members/approve",
            post(members::approve_member),
        )
        .route(
            "/circles/{circle_id}/members/reject",
            post(members::reject_member),
        )
        // Identity (global, no circle required)
        .route("/api/identity", get(identity::get_identity).post(identity::set_identity))
        .route("/api/identity/link", post(identity::link_device))
        .route("/api/identity/create-user", post(identity::create_user_identity))
        // M7 management
        .route("/api/init", post(management::init_circle))
        .route("/api/enter", post(management::enter_circle))
        .route(
            "/circles/{circle_id}/api/invite",
            post(management::generate_invite),
        )
        .route(
            "/circles/{circle_id}/api/enable",
            post(management::enable_circle),
        )
        .route(
            "/circles/{circle_id}/api/disable",
            post(management::disable_circle),
        )
        .route(
            "/circles/{circle_id}/api/leave",
            post(management::leave_circle),
        )
        .with_state(daemon)
        .layer(tower_http::cors::CorsLayer::permissive())
}
