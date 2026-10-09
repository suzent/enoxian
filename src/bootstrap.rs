//! Bootstrap server mode (`enox bootstrap serve`).
//!
//! The server holds no PSK and joins no Circle. It serves, over HTTP on
//! `--port`:
//! - `/version`: the running version, its capabilities, the oldest client it
//!   still serves (`min_client_version`) and its Iroh relay, if it runs one;
//! - `/peer-id`: the server's stable id from `bootstrap.key`, which the relay
//!   updater checks is unchanged after an update;
//! - `/pair`: the dead drop `enox link` uses;
//! - `/invite`: sealed short-invite blobs.
//!
//! With `--iroh-relay` (and a build with `iroh-relay-server`) it also runs the
//! Iroh relay Circles meet through. The libp2p rendezvous and circuit relay it
//! used to run are gone with libp2p.

use anyhow::{Context, Result};
use axum::{extract::State, routing::get, Json, Router};
use tracing::info;

use crate::{
    config::enoxian_dir,
    crypto::{generate_keypair, keypair_from_hex, keypair_to_hex},
    pair_mailbox::MailboxState,
};

/// The `--iroh-relay*` flags of `enox bootstrap serve`.
#[derive(Debug, Clone, Default)]
pub struct IrohRelayFlags {
    pub enabled: bool,
    pub dev_port: Option<u16>,
    pub acme_contact: Option<String>,
    pub acme_staging: bool,
}

/// Start the Iroh relay the flags ask for, returning the URL it serves.
#[cfg(feature = "iroh-relay-server")]
async fn start_iroh_relay(
    flags: IrohRelayFlags,
    advertise_host: Option<&str>,
) -> Result<Option<crate::iroh_relay_server::Relay>> {
    use crate::iroh_relay_server::{start, RelayMode};
    if !flags.enabled {
        return Ok(None);
    }
    let mode = match flags.dev_port {
        Some(port) => RelayMode::Dev { port },
        None => RelayMode::Production {
            domain: advertise_host
                .map(|h| h.trim().trim_end_matches('.').to_string())
                .filter(|h| !h.is_empty())
                .context("--iroh-relay needs --advertise-host: the certificate is issued for it")?,
            https_port: 443,
            http_port: 80,
            quic_port: Some(7842),
            contact: flags.acme_contact,
            staging: flags.acme_staging,
        },
    };
    Ok(Some(start(mode).await?))
}

#[cfg(not(feature = "iroh-relay-server"))]
async fn start_iroh_relay(
    flags: IrohRelayFlags,
    _advertise_host: Option<&str>,
) -> Result<Option<()>> {
    anyhow::ensure!(
        !flags.enabled,
        "--iroh-relay needs a build with the iroh-relay-server feature"
    );
    Ok(None)
}

pub async fn run(
    port: u16,
    advertise_host: Option<&str>,
    iroh_relay: IrohRelayFlags,
) -> Result<()> {
    // Before anything else binds: a relay that cannot start (port in use, no
    // hostname) should stop the server, not leave it half up. The relay lives
    // as long as this binding, which is the life of the server.
    let iroh_relay = start_iroh_relay(iroh_relay, advertise_host).await?;
    #[cfg(feature = "iroh-relay-server")]
    let iroh_relay_url = iroh_relay.as_ref().map(|relay| relay.url.clone());
    #[cfg(not(feature = "iroh-relay-server"))]
    let iroh_relay_url: Option<String> = {
        let _: Option<()> = iroh_relay;
        None
    };
    let keypair = load_or_create_keypair()?;
    let peer_id = keypair.public().to_peer_id().to_string();

    info!("Bootstrap server starting");
    info!("  PeerID : {peer_id}");
    info!("  HTTP   : http://0.0.0.0:{port}/version");

    let blobs = crate::invite_blobs::BlobState::new(enoxian_dir()?.join("invite-blobs"))?;
    // `/pair` is the dead drop two devices meet in during `enox link`. It is
    // mounted here because the bootstrap server is the one address both
    // machines already know how to reach; it is trusted with nothing — see
    // `crate::pair_mailbox`.
    let app = http_router(peer_id, iroh_relay_url, blobs);
    let http_addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(http_addr)
        .await
        .with_context(|| format!("binding the HTTP listener on {http_addr}"))?;
    // `into_make_service_with_connect_info` is what puts the peer address
    // within reach of the pairing mailbox's rate limit. Note it is the address
    // of whoever opened the socket: behind a reverse proxy every client looks
    // like the proxy, and `X-Forwarded-For` is deliberately not consulted,
    // because trusting it by default would let an attacker set it to whatever
    // they liked.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
    .context("HTTP server error")
}

#[derive(Clone)]
struct Meta {
    peer_id: String,
    /// The Iroh relay this server runs, when it runs one.
    iroh_relay: Option<String>,
}

/// Public bootstrap metadata and sealed-blob endpoints; no daemon credentials.
fn http_router(
    peer_id: String,
    iroh_relay: Option<String>,
    blobs: crate::invite_blobs::BlobState,
) -> Router {
    Router::new()
        .route("/peer-id", get(peer_id_handler))
        .route("/version", get(version_handler))
        .with_state(Meta {
            peer_id,
            iroh_relay,
        })
        .nest("/pair", crate::pair_mailbox::router(MailboxState::new()))
        .nest("/invite", crate::invite_blobs::router(blobs))
}

async fn version_handler(State(meta): State<Meta>) -> impl axum::response::IntoResponse {
    (
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "min_client_version": crate::defaults::MIN_CLIENT_VERSION,
            "iroh_relay": meta.iroh_relay,
            "capabilities": {
                "short_invites": true,
                "device_linking": true,
            },
        })),
    )
}

async fn peer_id_handler(State(meta): State<Meta>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "peer_id": meta.peer_id }))
}

fn load_or_create_keypair() -> Result<libp2p_identity::Keypair> {
    let dir = enoxian_dir()?;
    std::fs::create_dir_all(&dir).context("failed to create ~/.enoxian")?;
    let path = dir.join("bootstrap.key");
    if path.exists() {
        let hex = std::fs::read_to_string(&path).context("failed to read bootstrap.key")?;
        keypair_from_hex(hex.trim())
    } else {
        let keypair = generate_keypair();
        let hex = keypair_to_hex(&keypair)?;
        std::fs::write(&path, &hex).context("failed to write bootstrap.key")?;
        info!("Generated new bootstrap keypair → {}", path.display());
        Ok(keypair)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn version_is_public_uncached_and_preserves_peer_id_response() {
        use axum::{
            body::{to_bytes, Body},
            http::{Request, StatusCode},
        };
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        let blobs = crate::invite_blobs::BlobState::new(dir.path().to_owned()).unwrap();
        let app = http_router("test-peer".into(), None, blobs);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/version")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let json: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "min_client_version": crate::defaults::MIN_CLIENT_VERSION,
                "iroh_relay": null,
                "capabilities": { "short_invites": true, "device_linking": true },
            })
        );
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/peer-id")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let json: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(json, serde_json::json!({ "peer_id": "test-peer" }));
    }

    /// A server running an Iroh relay says where, so clients can find it.
    #[tokio::test]
    async fn version_advertises_the_iroh_relay() {
        use axum::{
            body::{to_bytes, Body},
            http::Request,
        };
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        let blobs = crate::invite_blobs::BlobState::new(dir.path().to_owned()).unwrap();
        let app = http_router(
            "test-peer".into(),
            Some("https://relay.example".into()),
            blobs,
        );
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/version")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(json["iroh_relay"], "https://relay.example");
    }
}
