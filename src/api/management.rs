use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;

use crate::{
    cli::{EnterArgs, InitArgs},
    commands::{enter, init},
    config,
    daemon::DaemonState,
    invite::{self, InvitePayload},
    crypto::keypair_from_hex,
};

#[derive(Deserialize)]
pub struct InitReq {
    pub name: String,
    pub dir: Option<String>,
}

#[derive(Deserialize)]
pub struct EnterReq {
    pub target: String,
    pub secret: Option<String>,
    pub peer: Option<String>,
    pub dir: Option<String>,
}

pub async fn init_circle(
    State(daemon): State<DaemonState>,
    Json(payload): Json<InitReq>,
) -> impl IntoResponse {
    let args = InitArgs {
        name: payload.name.clone(),
        ttl: "7d".to_string(),
        dir: payload.dir.map(std::path::PathBuf::from),
    };

    match init::run(args).await {
        Ok(_) => {
            if let Ok(configs) = config::load_all() {
                if let Some(c) = configs.into_iter().find(|c| c.circle_name == payload.name) {
                    let circle_id = c.circle_id.clone();
                    if !daemon.is_active(&circle_id) {
                        if let Err(e) = crate::lifecycle::spawn_circle(c, daemon).await {
                            tracing::warn!("[api] init_circle spawn failed: {e}");
                        }
                    }
                    return Json(json!({ "status": "ok", "circle_id": circle_id })).into_response();
                }
            }
            Json(json!({ "status": "ok" })).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

pub async fn enter_circle(
    State(daemon): State<DaemonState>,
    Json(payload): Json<EnterReq>,
) -> impl IntoResponse {
    // Extract circle_id from invite URI before running, so we can spawn it afterward.
    let circle_id_hint = if payload.target.starts_with("enochian://") {
        crate::invite::decode(&payload.target).ok().map(|p| p.circle_id)
    } else {
        Some(payload.target.clone())
    };

    let args = EnterArgs {
        target: payload.target,
        secret: payload.secret,
        peer: payload.peer,
        rendezvous: None,
        dir: payload.dir.map(std::path::PathBuf::from),
    };

    match enter::run(args).await {
        Ok(_) => {
            if let Some(id) = circle_id_hint {
                if let Ok(cfg) = config::load(&id) {
                    if !daemon.is_active(&id) {
                        if let Err(e) = crate::lifecycle::spawn_circle(cfg, daemon).await {
                            tracing::warn!("[api] enter_circle spawn failed: {e}");
                        }
                    }
                }
            }
            Json(json!({ "status": "ok" })).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

pub async fn generate_invite(
    Path(circle_id): Path<String>,
) -> impl IntoResponse {
    let configs = match config::load_all() {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    };
    
    let config = match configs.into_iter().find(|c| c.circle_id == circle_id) {
        Some(c) => c,
        None => return (StatusCode::NOT_FOUND, Json(json!({ "error": "circle not found" }))).into_response(),
    };

    let psk_bytes = match hex::decode(&config.psk_hex) {
        Ok(b) => b,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "invalid psk" }))).into_response(),
    };
    let psk: [u8; 32] = match psk_bytes.try_into() {
        Ok(p) => p,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "invalid psk length" }))).into_response(),
    };

    let ttl = std::time::Duration::from_secs(7 * 24 * 3600);
    let expires_at = chrono::Utc::now() + ttl;

    let admin_pubkey_bytes = config::circle_dir(&circle_id)
        .ok()
        .map(|d| d.join("admin.key"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|h| keypair_from_hex(h.trim()).ok())
        .map(|k| k.public().encode_protobuf());

    let uri = invite::encode(&InvitePayload {
        circle_id: config.circle_id.clone(),
        psk_bytes: psk,
        circle_name: Some(config.circle_name.clone()),
        expires_at,
        peer_addr: None,
        admin_pubkey_bytes,
        relay_addr: None,
        rendezvous_addr: None,
    });

    Json(json!({ "invite_uri": uri })).into_response()
}

pub async fn enable_circle(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
) -> impl IntoResponse {
    let mut config = match config::load(&circle_id) {
        Ok(c) => c,
        Err(_) => return (StatusCode::NOT_FOUND, Json(json!({ "error": "circle not found" }))).into_response(),
    };

    config.disabled = false;
    if let Err(e) = config::save(&config) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response();
    }

    // Try to start it
    if !daemon.is_active(&circle_id) {
        let _ = crate::lifecycle::spawn_circle(config, daemon).await;
    }

    Json(json!({ "status": "enabled" })).into_response()
}

pub async fn disable_circle(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
) -> impl IntoResponse {
    let mut config = match config::load(&circle_id) {
        Ok(c) => c,
        Err(_) => return (StatusCode::NOT_FOUND, Json(json!({ "error": "circle not found" }))).into_response(),
    };

    config.disabled = true;
    if let Err(e) = config::save(&config) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response();
    }

    daemon.stop_circle(&circle_id);

    Json(json!({ "status": "disabled" })).into_response()
}

pub async fn leave_circle(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
) -> impl IntoResponse {
    daemon.stop_circle(&circle_id);
    
    if let Ok(dir) = config::circle_dir(&circle_id) {
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    Json(json!({ "status": "left" })).into_response()
}
