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
    pub owner: Option<String>,
    pub join_policy: Option<String>,
}

#[derive(Deserialize)]
pub struct EnterReq {
    pub target: String,
    pub secret: Option<String>,
    pub peer: Option<String>,
    pub dir: Option<String>,
    pub owner: Option<String>,
}

pub async fn init_circle(
    State(daemon): State<DaemonState>,
    Json(payload): Json<InitReq>,
) -> impl IntoResponse {
    let args = InitArgs {
        name: payload.name.clone(),
        ttl: "7d".to_string(),
        dir: payload.dir.map(std::path::PathBuf::from),
        owner: payload.owner.clone(),
        join_policy: payload.join_policy.clone().unwrap_or_else(|| "auto".to_string()),
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
        owner: payload.owner,
    };

    let http_client = reqwest::Client::new();
    match enter::run(args, &http_client).await {
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
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
) -> impl IntoResponse {
    let config = match config::load(&circle_id) {
        Ok(c) => c,
        Err(_) => return (StatusCode::NOT_FOUND, Json(json!({ "error": "circle not found" }))).into_response(),
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

    // ── Auto-embed best peer address from live P2P state ──────────────────────
    // Priority: ExternalAddrConfirmed (confirmed by a peer via Identify)
    //           > best routable listen addr (public IP > Tailscale > RFC1918)
    // Falls back to None (LAN-only via mDNS) if daemon is down or has no addrs yet.
    let peer_addr = daemon.get(&circle_id).and_then(|state| {
        // Try external first (confirmed by a remote peer — most reliable for WAN)
        let ext = state.p2p_external_addrs.read().ok()?.first().cloned();
        if ext.is_some() { return ext; }
        // Fall back to best listen addr
        let listen = state.p2p_listen_addrs.read().ok()?;
        best_connectable_addr(listen.as_slice()).map(String::from)
    });

    // relay_addr and rendezvous_addr: from saved config (set when the admin
    // joined via a relay/rendezvous invite themselves).
    let relay_addr = config.relay_addrs.into_iter().next();
    let rendezvous_addr = config.rendezvous_addrs.into_iter().next();

    let uri = invite::encode(&InvitePayload {
        circle_id: config.circle_id.clone(),
        psk_bytes: psk,
        circle_name: Some(config.circle_name.clone()),
        expires_at,
        peer_addr: peer_addr.clone(),
        admin_pubkey_bytes,
        relay_addr: relay_addr.clone(),
        rendezvous_addr: rendezvous_addr.clone(),
    });

    Json(json!({
        "invite_uri": uri,
        // Tell the frontend what was embedded so it can show a connectivity hint
        "connectivity": {
            "peer_addr": peer_addr,
            "relay_addr": relay_addr,
            "rendezvous_addr": rendezvous_addr,
        }
    })).into_response()
}

/// Pick the best listen addr for embedding in an invite.
/// Prefers public IPs > Tailscale CGNAT (100.64/10) > RFC1918. Skips loopback / circuit addrs.
fn best_connectable_addr(addrs: &[String]) -> Option<&str> {
    fn rank(addr: &str) -> u8 {
        if addr.contains("/p2p-circuit") { return 5; }
        let ip_str = match addr.strip_prefix("/ip4/").and_then(|s| s.split('/').next()) {
            Some(s) => s,
            None => return 5,
        };
        let ip: std::net::Ipv4Addr = match ip_str.parse() {
            Ok(ip) => ip,
            Err(_) => return 5,
        };
        if ip.is_loopback() || ip.is_unspecified() { return 4; }
        if ip.is_private() || ip.is_link_local() { return 3; }
        let o = ip.octets();
        if o[0] == 100 && (64..=127).contains(&o[1]) { return 2; } // Tailscale
        1 // public IP
    }
    addrs.iter()
        .filter(|a| rank(a) < 4)
        .min_by_key(|a| rank(a))
        .map(String::as_str)
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
