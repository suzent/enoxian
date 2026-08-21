pub mod agent_config;
pub mod auth;
pub mod chat;
pub mod connectivity;
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
    extract::State,
    routing::{get, post},
    Json, Router,
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

/// Build the API router. `token`, when `Some`, is required on every request as
/// `Authorization: Bearer <token>` (or `?token=` for WebSocket/SSE, which cannot
/// set headers). `None` disables auth — only for tests.
pub fn router(daemon: DaemonState, token: Option<String>) -> Router {
    let base = Router::new()
        .route("/shutdown", post(shutdown::shutdown))
        .route("/circles", get(list_circles))
        .route("/circles/{circle_id}/ws/yjs", get(ws_yjs_handler))
        .route("/circles/{circle_id}/api/status", get(status::get_status))
        .route(
            "/circles/{circle_id}/api/connectivity",
            get(connectivity::get_connectivity).post(connectivity::set_connectivity),
        )
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
        .route(
            "/circles/{circle_id}/api/chat/activity",
            get(chat::get_activity).post(chat::post_activity),
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
        .route("/api/agent-config", get(agent_config::get_agent_config))
        .route(
            "/api/agent-config/discover",
            get(agent_config::discover_agents),
        )
        .route("/api/agent-plugins", get(agent_config::list_plugins))
        .route(
            "/api/agent-plugins/{plugin_id}/install",
            post(agent_config::install_plugin),
        )
        .route(
            "/api/agent-config/reaction",
            post(agent_config::set_reaction),
        )
        .route("/api/agent-config/agents", post(agent_config::add_agent))
        .route(
            "/api/agent-config/agents/remove",
            post(agent_config::remove_agent),
        )
        .route(
            "/api/identity",
            get(identity::get_identity).post(identity::set_identity),
        )
        .route("/api/identity/link", post(identity::link_device))
        .route(
            "/api/identity/create-user",
            post(identity::create_user_identity),
        )
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
        .with_state(daemon);

    // CORS: allow only local origins (the frontend served from this daemon, and
    // localhost dev servers). A permissive policy would let any website's
    // scripts read authenticated responses from this control plane.
    let cors = tower_http::cors::CorsLayer::new()
        .allow_origin(tower_http::cors::AllowOrigin::predicate(|origin, _req| {
            origin.to_str().map(is_local_origin).unwrap_or(false)
        }))
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any);

    // Token auth via `route_layer`: applies ONLY to the routes defined in this
    // router, so when the caller merges an (un-authed) frontend router the static
    // assets are not affected. `None` (tests only) skips auth.
    let base = match token {
        Some(t) => base.route_layer(axum::middleware::from_fn_with_state(
            std::sync::Arc::new(t),
            auth::require_token,
        )),
        None => base,
    };

    base.layer(cors)
}

/// Whether an `Origin` header value is a local address (loopback host, any
/// port/scheme). Used by the CORS allowlist.
fn is_local_origin(origin: &str) -> bool {
    // Strip scheme.
    let host = origin
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(origin);
    // Strip port.
    let host = host.split(':').next().unwrap_or(host);
    matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1")
}
