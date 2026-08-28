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
    crypto::keypair_from_hex,
    daemon::DaemonState,
    invite::{self, InvitePayload},
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
        join_policy: payload
            .join_policy
            .clone()
            .unwrap_or_else(|| "auto".to_string()),
    };

    match init::run(args).await {
        Ok(_) => {
            if let Ok(configs) = config::load_all() {
                if let Some(c) = configs.into_iter().find(|c| c.circle_name == payload.name) {
                    let circle_id = c.circle_id.clone();
                    if !daemon.is_active(&circle_id) {
                        // Run in a separate task so a panic inside spawn_circle becomes a
                        // JoinError rather than propagating to the handler as a 500.
                        // We still await the handle so the circle is registered before we
                        // respond — the frontend's reloadCircles() will see it immediately.
                        match tokio::spawn(crate::lifecycle::spawn_circle(c, daemon)).await {
                            Ok(Err(e)) => tracing::warn!("[api] init_circle spawn failed: {e}"),
                            Err(e) => tracing::warn!("[api] init_circle spawn panicked: {e}"),
                            Ok(Ok(())) => {}
                        }
                    }
                    return Json(json!({ "status": "ok", "circle_id": circle_id })).into_response();
                }
            }
            Json(json!({ "status": "ok" })).into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

pub async fn enter_circle(
    State(daemon): State<DaemonState>,
    Json(payload): Json<EnterReq>,
) -> impl IntoResponse {
    // Extract circle_id from invite URI before running, so we can spawn it afterward.
    let circle_id_hint = if payload.target.starts_with("enoxian://") {
        crate::invite::decode(&payload.target)
            .ok()
            .map(|p| p.circle_id)
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
        // Skip the 10-second connectivity verification step — the daemon's P2P
        // swarm spawned below handles connectivity in the background.
        no_verify: true,
    };

    let http_client = reqwest::Client::new();
    match enter::run(args, &http_client).await {
        Ok(_) => {
            let resolved_circle_id = circle_id_hint.as_deref().and_then(|hint| {
                config::load(hint)
                    .ok()
                    .map(|cfg| cfg.circle_id)
                    .or_else(|| {
                        config::load_all()
                            .ok()?
                            .into_iter()
                            .find(|cfg| cfg.circle_id.starts_with(hint) || cfg.circle_name == hint)
                            .map(|cfg| cfg.circle_id)
                    })
            });
            if let Some(id) = resolved_circle_id.as_deref() {
                if let Ok(cfg) = config::load(id) {
                    if !daemon.is_active(id) {
                        match tokio::spawn(crate::lifecycle::spawn_circle(cfg, daemon)).await {
                            Ok(Err(e)) => tracing::warn!("[api] enter_circle spawn failed: {e}"),
                            Err(e) => tracing::warn!("[api] enter_circle spawn panicked: {e}"),
                            Ok(Ok(())) => {}
                        }
                    }
                }
            }
            Json(json!({ "status": "ok", "circle_id": resolved_circle_id })).into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

pub async fn generate_invite(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
) -> impl IntoResponse {
    let config = match config::load(&circle_id) {
        Ok(c) => c,
        Err(_) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "circle not found" })),
            )
                .into_response()
        }
    };

    let psk_bytes = match hex::decode(&config.psk_hex) {
        Ok(b) => b,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "invalid psk" })),
            )
                .into_response()
        }
    };
    let psk: [u8; 32] = match psk_bytes.try_into() {
        Ok(p) => p,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "invalid psk length" })),
            )
                .into_response()
        }
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
    // Priority:
    //   1. ExternalAddrConfirmed (confirmed by a peer via Identify) — most reliable for WAN
    //   2. Best routable listen addr (public IP > Tailscale > RFC1918)
    //   3. Relay circuit address (relay_addr/p2p-circuit/p2p/OUR_PEER_ID) — works
    //      for NAT'd peers with a relay reservation; joiner dials us via relay.
    let peer_addr = daemon.get(&circle_id).and_then(|state| {
        // Try external first (confirmed by a remote peer — most reliable for WAN)
        let ext = state.p2p_external_addrs.read().ok()?.first().cloned();
        if ext.is_some() {
            return ext;
        }
        // Fall back to best listen addr
        let listen = state.p2p_listen_addrs.read().ok()?;
        best_connectable_addr(listen.as_slice()).map(String::from)
    });

    // relay_addr: from saved config, or fall back to the default relay server
    // so invites are usable for WAN NAT traversal even without manual configuration.
    // Resolved first so it can also serve as the peer_addr fallback below.
    let relay_addr = if let Some(saved) = config.relay_addrs.into_iter().next() {
        Some(saved)
    } else {
        crate::commands::rendezvous::resolve_default_relay().await
    };

    // If no direct/external address is available (NAT'd peer, daemon just started),
    // derive our relay circuit address from relay_addr + our keypair's peer ID.
    // This is deterministic and reachable: the joiner dials us through the relay.
    let peer_addr = if peer_addr.is_none() {
        relay_addr
            .as_deref()
            .and_then(|relay_str| relay_str.parse::<libp2p::Multiaddr>().ok())
            .and_then(|relay_maddr| {
                keypair_from_hex(&config.keypair_proto_hex).ok().map(|kp| {
                    let my_peer_id = kp.public().to_peer_id();
                    relay_maddr
                        .with(libp2p::multiaddr::Protocol::P2pCircuit)
                        .with(libp2p::multiaddr::Protocol::P2p(my_peer_id))
                        .to_string()
                })
            })
    } else {
        peer_addr
    };

    // rendezvous_addr: from saved config, or fall back to the default server
    // (enoxian.com) so invites are WAN-capable even without manual configuration.
    let rendezvous_addr = if let Some(saved) = config.rendezvous_addrs.into_iter().next() {
        Some(saved)
    } else {
        crate::commands::rendezvous::resolve_default().await
    };

    // Sign the invite with this member's own circle key. Every member has one,
    // so any member can still invite; the grant records which of them did.
    let grant = invite::sign_grant(&config.circle_id, &config.keypair_proto_hex, expires_at)
        .map_err(|e| {
            tracing::warn!("[invite] could not sign invite grant: {e}");
            e
        })
        .ok();
    let uri = invite::encode(&InvitePayload {
        circle_id: config.circle_id.clone(),
        psk_bytes: psk,
        circle_name: Some(config.circle_name.clone()),
        expires_at,
        peer_addr: peer_addr.clone(),
        admin_pubkey_bytes,
        relay_addr: relay_addr.clone(),
        rendezvous_addr: rendezvous_addr.clone(),
        grant,
    });

    Json(json!({
        "invite_uri": uri,
        // Tell the frontend what was embedded so it can show a connectivity hint
        "connectivity": {
            "peer_addr": peer_addr,
            "relay_addr": relay_addr,
            "rendezvous_addr": rendezvous_addr,
        }
    }))
    .into_response()
}

/// Pick the best listen addr for embedding in an invite.
/// Prefers public IPs > Tailscale CGNAT (100.64/10) > RFC1918. Skips loopback / circuit addrs.
fn best_connectable_addr(addrs: &[String]) -> Option<&str> {
    fn rank(addr: &str) -> u8 {
        if addr.contains("/p2p-circuit") {
            return 5;
        }
        let ip_str = match addr.strip_prefix("/ip4/").and_then(|s| s.split('/').next()) {
            Some(s) => s,
            None => return 5,
        };
        let ip: std::net::Ipv4Addr = match ip_str.parse() {
            Ok(ip) => ip,
            Err(_) => return 5,
        };
        if ip.is_loopback() || ip.is_unspecified() {
            return 4;
        }
        if ip.is_private() || ip.is_link_local() {
            return 3;
        }
        let o = ip.octets();
        if o[0] == 100 && (64..=127).contains(&o[1]) {
            return 2;
        } // Tailscale
        1 // public IP
    }
    addrs
        .iter()
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
        Err(_) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "circle not found" })),
            )
                .into_response()
        }
    };

    config.disabled = false;
    if let Err(e) = config::save(&config) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response();
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
        Err(_) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "circle not found" })),
            )
                .into_response()
        }
    };

    config.disabled = true;
    if let Err(e) = config::save(&config) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response();
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
            if let Err(e) = std::fs::remove_dir_all(&dir) {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "error": format!("failed to remove local Circle configuration: {e}")
                    })),
                )
                    .into_response();
            }
        }
    }

    Json(json!({ "status": "left" })).into_response()
}
