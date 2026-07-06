//! Bootstrap server mode (`enoxd --bootstrap`).
//!
//! Runs a public rendezvous + circuit-relay node that circle members can use
//! for WAN peer discovery.  The server holds no PSK and joins no circle.
//! It speaks QUIC (no PSK) for rendezvous and TCP (no PSK) for circuit relay.
//!
//! # Setup (enox.suzent.com)
//!
//! 1. On the server: `enoxd --bootstrap --port 36521 --relay-port 36522`
//!    Copy the printed peer ID from the log.
//! 2. Share the multiaddr with circle members:
//!    `/ip4/<PUBLIC_IP>/udp/36521/quic-v1/p2p/<PEER_ID>`
//! 3. Members pass it via `enox enter <invite> --rendezvous <addr>`
//!    or embed it in invites with `enox invite --rendezvous <addr>`.

use anyhow::{Context, Result};
use axum::{extract::State, routing::get, Json, Router};
use libp2p::{
    core::{muxing::StreamMuxerBox, upgrade},
    futures::future::Either,
    futures::StreamExt,
    identify, kad, noise, quic, relay, rendezvous, tcp, yamux,
    swarm::SwarmEvent,
    Multiaddr, SwarmBuilder, Transport,
};
use tracing::info;

use crate::{
    config::enoxian_dir,
    crypto::{generate_keypair, keypair_from_hex, keypair_to_hex},
    network::bootstrap_behaviour::{BootstrapBehaviour, BootstrapEvent},
};

pub async fn run(port: u16, relay_port: u16) -> Result<()> {
    let keypair = load_or_create_keypair()?;
    let peer_id = keypair.public().to_peer_id();
    let peer_id_str = peer_id.to_string();

    info!("Bootstrap server starting");
    info!("  PeerID : {peer_id}");
    info!("  HTTP   : http://0.0.0.0:{port}/peer-id  (for enox CLI auto-resolution)");
    info!("  Relay  : tcp/0.0.0.0:{relay_port}");
    info!("  Share once you see the QUIC and TCP listen addresses below.");

    let mut swarm = SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_other_transport(|key| {
            let tcp = tcp::tokio::Transport::new(tcp::Config::default())
                .upgrade(upgrade::Version::V1Lazy)
                .authenticate(noise::Config::new(key)?)
                .multiplex(yamux::Config::default())
                .map(|(id, muxer), _| (id, StreamMuxerBox::new(muxer)));

            let quic_t = quic::tokio::Transport::new(quic::Config::new(key))
                .map(|(id, muxer), _| (id, StreamMuxerBox::new(muxer)));

            Ok(libp2p::dns::tokio::Transport::system(
                tcp.or_transport(quic_t).map(|e, _| match e {
                    Either::Left(x) => x,
                    Either::Right(x) => x,
                })
            )?.map(|(id, muxer), _| (id, StreamMuxerBox::new(muxer))))
        })?
        .with_behaviour(|key| {
            let pid = key.public().to_peer_id();
            Ok(BootstrapBehaviour {
                rendezvous: rendezvous::server::Behaviour::new(
                    rendezvous::server::Config::default()
                        .with_max_ttl(2 * 3600)
                        .with_min_ttl(60),
                ),
                relay: relay::Behaviour::new(pid, relay::Config::default()),
                identify: identify::Behaviour::new(identify::Config::new(
                    "/enoxian-bootstrap/1.0.0".to_string(),
                    key.public(),
                )),
                ping: libp2p::ping::Behaviour::default(),
                kad: {
                    let mut k = kad::Behaviour::new(pid, kad::store::MemoryStore::new(pid));
                    k.set_mode(Some(kad::Mode::Server));
                    k
                },
            })
        })?
        .build();

    let rendezvous_listen_addr: Multiaddr = format!("/ip4/0.0.0.0/udp/{port}/quic-v1").parse()?;
    let relay_listen_addr: Multiaddr = format!("/ip4/0.0.0.0/tcp/{relay_port}").parse()?;
    swarm.listen_on(rendezvous_listen_addr)?;
    swarm.listen_on(relay_listen_addr)?;

    // ── HTTP server: GET /peer-id — allows `enox` CLI to auto-resolve the ──────
    // full multiaddr without the operator having to copy-paste the peer ID.
    // Runs on TCP:<port> alongside QUIC on UDP:<port> — no conflict.
    let app = Router::new()
        .route("/peer-id", get(peer_id_handler))
        .with_state(peer_id_str.clone());
    let http_addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(http_addr).await
            .expect("failed to bind HTTP listener");
        axum::serve(listener, app).await
            .expect("HTTP server error");
    });

    loop {
        match swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { address, .. } => {
                info!("Bootstrap listening on {address}");
                if address.to_string().contains("/udp/") {
                    info!("  Rendezvous address for circle members:");
                } else {
                    info!("  Relay address for circle members:");
                }
                info!("    {address}/p2p/{peer_id}");
                info!("  Or just run: enox invite <circle> --rendezvous <hostname>");
            }
            SwarmEvent::ConnectionEstablished { peer_id: remote, endpoint, .. } => {
                info!("[bootstrap] peer connected: {remote} via {}", endpoint.get_remote_address());
            }
            SwarmEvent::ConnectionClosed { peer_id: remote, cause, .. } => {
                info!("[bootstrap] peer disconnected: {remote}: {cause:?}");
            }
            SwarmEvent::Behaviour(BootstrapEvent::Rendezvous(e)) => {
                use rendezvous::server::Event;
                match &e {
                    Event::PeerRegistered { peer, registration } => {
                        info!("[rendezvous] registered: {peer} ns={}", registration.namespace);
                    }
                    Event::PeerUnregistered { peer, namespace } => {
                        info!("[rendezvous] unregistered: {peer} ns={namespace}");
                    }
                    Event::DiscoverServed { enquirer, registrations } => {
                        info!("[rendezvous] served {} registrations to {enquirer}", registrations.len());
                    }
                    _ => tracing::debug!("[rendezvous] {e:?}"),
                }
            }
            SwarmEvent::Behaviour(BootstrapEvent::Relay(e)) => {
                tracing::debug!("[relay] {e:?}");
            }
            SwarmEvent::Behaviour(BootstrapEvent::Identify(identify::Event::Received {
                peer_id: remote, info, ..
            })) => {
                for addr in &info.listen_addrs {
                    swarm.behaviour_mut().kad.add_address(&remote, addr.clone());
                }
            }
            SwarmEvent::OutgoingConnectionError { error, .. } => {
                tracing::debug!("[bootstrap] outgoing error: {error}");
            }
            _ => {}
        }
    }
}

async fn peer_id_handler(State(peer_id): State<String>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "peer_id": peer_id }))
}

fn load_or_create_keypair() -> Result<libp2p::identity::Keypair> {
    let dir = enoxian_dir()?;
    std::fs::create_dir_all(&dir).context("failed to create ~/.enoxian")?;
    let path = dir.join("bootstrap.key");
    if path.exists() {
        let hex = std::fs::read_to_string(&path)
            .context("failed to read bootstrap.key")?;
        keypair_from_hex(hex.trim())
    } else {
        let keypair = generate_keypair();
        let hex = keypair_to_hex(&keypair)?;
        std::fs::write(&path, &hex)
            .context("failed to write bootstrap.key")?;
        info!("Generated new bootstrap keypair → {}", path.display());
        Ok(keypair)
    }
}
