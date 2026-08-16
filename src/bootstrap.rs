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
    identify, kad, noise, quic, relay, rendezvous,
    swarm::SwarmEvent,
    tcp, yamux, Multiaddr, SwarmBuilder, Transport,
};
use std::{num::NonZeroU32, time::Duration};
use tracing::{info, warn};

use crate::{
    config::enoxian_dir,
    crypto::{generate_keypair, keypair_from_hex, keypair_to_hex},
    network::bootstrap_behaviour::{BootstrapBehaviour, BootstrapEvent},
};

pub async fn run(port: u16, relay_port: u16, advertise_host: Option<&str>) -> Result<()> {
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

            Ok(
                libp2p::dns::tokio::Transport::system(tcp.or_transport(quic_t).map(
                    |e, _| match e {
                        Either::Left(x) => x,
                        Either::Right(x) => x,
                    },
                ))?
                .map(|(id, muxer), _| (id, StreamMuxerBox::new(muxer))),
            )
        })?
        .with_behaviour(|key| {
            let pid = key.public().to_peer_id();
            Ok(BootstrapBehaviour {
                rendezvous: rendezvous::server::Behaviour::new(
                    rendezvous::server::Config::default()
                        .with_max_ttl(2 * 3600)
                        .with_min_ttl(60),
                ),
                relay: relay::Behaviour::new(pid, relay_server_config()),
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

    if let Some(host) = advertise_host {
        let host = host.trim().trim_end_matches('.');
        anyhow::ensure!(!host.is_empty(), "--advertise-host cannot be empty");

        let rendezvous_addr: Multiaddr = format!("/dns4/{host}/udp/{port}/quic-v1")
            .parse()
            .with_context(|| format!("invalid advertised hostname '{host}'"))?;
        let relay_addr: Multiaddr = format!("/dns4/{host}/tcp/{relay_port}")
            .parse()
            .with_context(|| format!("invalid advertised hostname '{host}'"))?;

        info!("  Advertise: {rendezvous_addr}/p2p/{peer_id}");
        info!("             {relay_addr}/p2p/{peer_id}");
        swarm.add_external_address(rendezvous_addr);
        swarm.add_external_address(relay_addr);
    }

    // ── HTTP server: GET /peer-id — allows `enox` CLI to auto-resolve the ──────
    // full multiaddr without the operator having to copy-paste the peer ID.
    // Runs on TCP:<port> alongside QUIC on UDP:<port> — no conflict.
    let app = Router::new()
        .route("/peer-id", get(peer_id_handler))
        .with_state(peer_id_str.clone());
    let http_addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(http_addr)
            .await
            .expect("failed to bind HTTP listener");
        axum::serve(listener, app).await.expect("HTTP server error");
    });

    loop {
        match swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { address, .. } => {
                info!("Bootstrap listening on {address}");
                if is_public_listen_addr(&address) {
                    swarm.add_external_address(address.clone());
                }
                if address.to_string().contains("/udp/") {
                    info!("  Rendezvous address for circle members:");
                } else {
                    info!("  Relay address for circle members:");
                }
                info!("    {address}/p2p/{peer_id}");
                info!("  Or just run: enox invite <circle> --rendezvous <hostname>");
            }
            SwarmEvent::ConnectionEstablished {
                peer_id: remote,
                endpoint,
                ..
            } => {
                info!(
                    "[bootstrap] peer connected: {remote} via {}",
                    endpoint.get_remote_address()
                );
            }
            SwarmEvent::ConnectionClosed {
                peer_id: remote,
                cause,
                ..
            } => {
                info!("[bootstrap] peer disconnected: {remote}: {cause:?}");
            }
            SwarmEvent::Behaviour(BootstrapEvent::Rendezvous(e)) => {
                use rendezvous::server::Event;
                match &e {
                    Event::PeerRegistered { peer, registration } => {
                        info!(
                            "[rendezvous] registered: {peer} ns={}",
                            registration.namespace
                        );
                    }
                    Event::PeerUnregistered { peer, namespace } => {
                        info!("[rendezvous] unregistered: {peer} ns={namespace}");
                    }
                    Event::DiscoverServed {
                        enquirer,
                        registrations,
                    } => {
                        info!(
                            "[rendezvous] served {} registrations to {enquirer}",
                            registrations.len()
                        );
                    }
                    _ => tracing::debug!("[rendezvous] {e:?}"),
                }
            }
            SwarmEvent::Behaviour(BootstrapEvent::Relay(e)) => {
                use relay::Event;
                match &e {
                    Event::ReservationReqAccepted {
                        src_peer_id,
                        renewed,
                    } => {
                        info!("[relay] reservation accepted: peer={src_peer_id} renewed={renewed}");
                    }
                    Event::ReservationReqDenied {
                        src_peer_id,
                        status,
                    } => {
                        warn!("[relay] reservation denied: peer={src_peer_id} status={status:?}");
                    }
                    Event::ReservationClosed { src_peer_id } => {
                        info!("[relay] reservation closed: peer={src_peer_id}");
                    }
                    Event::ReservationTimedOut { src_peer_id } => {
                        info!("[relay] reservation timed out: peer={src_peer_id}");
                    }
                    Event::CircuitReqAccepted {
                        src_peer_id,
                        dst_peer_id,
                    } => {
                        info!("[relay] circuit accepted: src={src_peer_id} dst={dst_peer_id}");
                    }
                    Event::CircuitReqDenied {
                        src_peer_id,
                        dst_peer_id,
                        status,
                    } => {
                        warn!("[relay] circuit denied: src={src_peer_id} dst={dst_peer_id} status={status:?}");
                    }
                    Event::CircuitClosed {
                        src_peer_id,
                        dst_peer_id,
                        error,
                    } => {
                        if let Some(error) = error {
                            warn!("[relay] circuit closed: src={src_peer_id} dst={dst_peer_id} error={error}");
                        } else {
                            info!("[relay] circuit closed: src={src_peer_id} dst={dst_peer_id}");
                        }
                    }
                    _ => tracing::debug!("[relay] {e:?}"),
                }
            }
            SwarmEvent::Behaviour(BootstrapEvent::Identify(identify::Event::Received {
                peer_id: remote,
                info,
                ..
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

fn relay_server_config() -> relay::Config {
    // Enoxian keeps sync and presence streams alive; libp2p's generic defaults
    // (16 circuits, 2 minutes, 128 KiB) cause reconnect churn under normal use.
    let mut config = relay::Config {
        max_reservations: 512,
        max_reservations_per_peer: 8,
        max_circuits: 128,
        max_circuits_per_peer: 16,
        max_circuit_duration: Duration::from_secs(30 * 60),
        max_circuit_bytes: 64 * 1024 * 1024,
        ..Default::default()
    };

    // Keep abuse protection, but permit short reconnect bursts after a network
    // transition. The stock bucket only refills one request every two minutes.
    config.circuit_src_rate_limiters.clear();
    config = config.circuit_src_per_peer(
        NonZeroU32::new(64).expect("64 is non-zero"),
        Duration::from_secs(1),
    );
    config.circuit_src_per_ip(
        NonZeroU32::new(128).expect("128 is non-zero"),
        Duration::from_secs(1),
    )
}

fn is_public_listen_addr(addr: &Multiaddr) -> bool {
    use libp2p::multiaddr::Protocol;

    addr.iter().any(|protocol| match protocol {
        Protocol::Ip4(ip) => {
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && !ip.is_broadcast()
                && !ip.is_documentation()
        }
        Protocol::Ip6(ip) => is_public_ipv6(ip),
        _ => false,
    })
}

fn is_public_ipv6(ip: std::net::Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    let is_unique_local = first & 0xfe00 == 0xfc00; // fc00::/7
    let is_unicast_link_local = first & 0xffc0 == 0xfe80; // fe80::/10
    !ip.is_loopback()
        && !ip.is_unspecified()
        && !ip.is_multicast()
        && !is_unique_local
        && !is_unicast_link_local
}

async fn peer_id_handler(State(peer_id): State<String>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "peer_id": peer_id }))
}

fn load_or_create_keypair() -> Result<libp2p::identity::Keypair> {
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

    fn addr(value: &str) -> Multiaddr {
        value.parse().unwrap()
    }

    #[test]
    fn public_ipv6_is_advertised() {
        assert!(is_public_listen_addr(&addr(
            "/ip6/2606:4700:4700::1111/tcp/36521"
        )));
    }

    #[test]
    fn private_ipv6_ranges_are_not_advertised() {
        for value in [
            "/ip6/::1/tcp/36521",
            "/ip6/::/tcp/36521",
            "/ip6/fc00::1/tcp/36521",
            "/ip6/fd12:3456::1/tcp/36521",
            "/ip6/fe80::1/tcp/36521",
            "/ip6/ff02::1/tcp/36521",
        ] {
            assert!(!is_public_listen_addr(&addr(value)), "{value}");
        }
    }
}
