use axum::{extract::{Path, State}, http::StatusCode, response::IntoResponse, Json};
use serde_json::json;
use crate::{config, daemon::DaemonState};

pub async fn get_status(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => return (StatusCode::NOT_FOUND, Json(json!({"error": "circle not found"}))).into_response(),
    };
    let workspace = state.workspace.clone();
    let conflicts = tokio::task::spawn_blocking(move || {
        crate::store::conflicts::scan_conflicts(&workspace)
    }).await.unwrap_or_default();

    let external_addrs = state.p2p_external_addrs
        .read()
        .map(|v| v.clone())
        .unwrap_or_default();

    let listen_addrs = state.p2p_listen_addrs
        .read()
        .map(|v| v.clone())
        .unwrap_or_default();

    // relay_addrs and rendezvous_addrs come from the persisted circle config.
    let (relay_addrs, rendezvous_addrs) = config::load(&state.circle_id)
        .map(|c| (c.relay_addrs, c.rendezvous_addrs))
        .unwrap_or_default();

    // Recent connection failures (most recent first) — makes silent handshake
    // failures like a PSK mismatch visible without trawling daemon logs.
    let conn_errors: Vec<_> = state.recent_conn_errors
        .read()
        .map(|q| q.iter().rev().map(|(ts, msg)| json!({"ts": ts, "error": msg})).collect())
        .unwrap_or_default();

    Json(json!({
        "circle_id":   state.circle_id,
        "circle_name": state.circle_name,
        "workspace":   state.workspace.to_string_lossy(),
        "agent_id":    state.agent_id,
        "docs":        state.docs.len(),
        "conflicts":   conflicts,
        "p2p": {
            "peer_id":          state.peer_id,
            "external_addrs":   external_addrs,
            "listen_addrs":     listen_addrs,
            "relay_addrs":      relay_addrs,
            "rendezvous_addrs": rendezvous_addrs,
            "recent_conn_errors": conn_errors,
        },
    })).into_response()
}
