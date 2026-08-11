use crate::{config, daemon::DaemonState, lifecycle};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
pub struct ConnectivityUpdate {
    force_relay: bool,
}

pub async fn get_connectivity(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
) -> impl IntoResponse {
    match config::load(&circle_id) {
        Ok(cfg) => Json(json!({
            "force_relay": cfg.force_relay,
            "active": daemon.is_active(&cfg.circle_id),
            "relay_configured": !cfg.relay_addrs.is_empty()
                || crate::defaults::DEFAULT_RELAY.is_some(),
            "rendezvous_configured": !cfg.rendezvous_addrs.is_empty()
                || crate::defaults::DEFAULT_RENDEZVOUS.is_some(),
        }))
        .into_response(),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

pub async fn set_connectivity(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Json(update): Json<ConnectivityUpdate>,
) -> impl IntoResponse {
    let mut cfg = match config::load(&circle_id) {
        Ok(cfg) => cfg,
        Err(e) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    };

    if cfg.force_relay == update.force_relay {
        return Json(json!({
            "force_relay": cfg.force_relay,
            "active": daemon.is_active(&cfg.circle_id),
            "restarted": false,
        }))
        .into_response();
    }

    let previous = cfg.force_relay;
    cfg.force_relay = update.force_relay;
    if let Err(e) = config::save(&cfg) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let was_active = daemon.stop_circle(&cfg.circle_id);
    if was_active {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if let Err(e) = lifecycle::spawn_circle(cfg.clone(), daemon.clone()).await {
            tracing::warn!(
                "[connectivity] failed to restart circle {} in force_relay={} mode: {e}",
                cfg.circle_id,
                cfg.force_relay
            );

            cfg.force_relay = previous;
            let rollback_save = config::save(&cfg).err();
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            let rollback_start = lifecycle::spawn_circle(cfg, daemon).await.err();
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": format!("failed to switch connectivity mode: {e}"),
                    "rolled_back": rollback_save.is_none() && rollback_start.is_none(),
                })),
            )
                .into_response();
        }
    }

    Json(json!({
        "force_relay": update.force_relay,
        "active": was_active,
        "restarted": was_active,
    }))
    .into_response()
}
