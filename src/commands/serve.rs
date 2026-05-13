use anyhow::{Context, Result};
use libp2p::{
    futures::StreamExt,
    identify, kad, mdns, noise, tcp, yamux,
    swarm::{dial_opts::DialOpts, SwarmEvent},
    Multiaddr, SwarmBuilder,
};
use std::net::SocketAddr;
use tracing::{info, warn};

use crate::{
    api,
    cli::ServeArgs,
    config::load,
    crypto::keypair_from_hex,
    network::behaviour::{EnochBehaviour, EnochEvent},
    state::AppState,
    sync_yjs::watcher::spawn_watcher,
};

pub async fn run(args: ServeArgs) -> Result<()> {
    let config = load(&args.circle).context("circle not found — run `enoch init` first")?;
    let keypair = keypair_from_hex(&config.keypair_proto_hex)?;
    let peer_id = keypair.public().to_peer_id();

    // Sync directory: ~/.enochian/circles/<id>/files  (or --sync-dir override)
    let sync_dir = match args.sync_dir {
        Some(d) => d,
        None => crate::config::circle_dir(&config.circle_id)?.join("files"),
    };
    tokio::fs::create_dir_all(&sync_dir).await?;

    info!("Starting enochd for circle '{}' ({})", config.circle_name, config.circle_id);
    info!("PeerID:   {peer_id}");
    info!("SyncDir:  {}", sync_dir.display());

    let state = AppState::new(
        config.circle_id.clone(),
        config.circle_name.clone(),
        sync_dir.clone(),
    );

    // ── File watcher (Phase 1) ────────────────────────────────────────────
    spawn_watcher(state.clone(), sync_dir).await?;

    // ── libp2p swarm (Phase 0) ────────────────────────────────────────────
    let mut swarm = SwarmBuilder::with_existing_identity(keypair.clone())
        .with_tokio()
        .with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default)?
        .with_behaviour(|key| {
            let peer_id = key.public().to_peer_id();
            Ok(EnochBehaviour {
                mdns: mdns::tokio::Behaviour::new(mdns::Config::default(), peer_id)?,
                kad: {
                    let mut kad = kad::Behaviour::new(
                        peer_id,
                        kad::store::MemoryStore::new(peer_id),
                    );
                    kad.set_mode(Some(kad::Mode::Server));
                    kad
                },
                identify: identify::Behaviour::new(identify::Config::new(
                    "/enochian/1.0.0".to_string(),
                    key.public(),
                )),
                ping: libp2p::ping::Behaviour::default(),
                rendezvous: libp2p::rendezvous::client::Behaviour::new(keypair.clone()),
            })
        })?
        .build();

    // P2P listens on port+1 so it doesn't conflict with the HTTP port
    let p2p_port = args.port + 1;
    let p2p_addr: Multiaddr = format!("/ip4/0.0.0.0/tcp/{p2p_port}")
        .parse()
        .context("invalid p2p addr")?;
    swarm.listen_on(p2p_addr)?;

    // ── axum HTTP + WS server (Phase 1) ───────────────────────────────────
    let app = api::router(state);
    let http_addr = SocketAddr::from(([0, 0, 0, 0], args.port));
    let listener = tokio::net::TcpListener::bind(http_addr).await
        .with_context(|| format!("failed to bind HTTP server on :{}", args.port))?;
    info!("HTTP/WS listening on :{} (P2P on :{})", args.port, p2p_port);

    // Run swarm + axum concurrently
    tokio::select! {
        result = axum::serve(listener, app) => {
            result.context("axum server error")?;
        }
        _ = async {
            loop {
                match swarm.select_next_some().await {
                    SwarmEvent::NewListenAddr { address, .. } => {
                        info!("P2P listening on {address}");
                    }
                    SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                        info!("P2P connected: {peer_id} via {}", endpoint.get_remote_address());
                    }
                    SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                        info!("P2P disconnected: {peer_id}: {cause:?}");
                    }
                    SwarmEvent::Behaviour(EnochEvent::Mdns(mdns::Event::Discovered(peers))) => {
                        for (peer_id, addr) in peers {
                            info!("mDNS discovered: {peer_id} @ {addr}");
                            swarm.behaviour_mut().kad.add_address(&peer_id, addr.clone());
                            if swarm.is_connected(&peer_id) { continue; }
                            if let Err(e) = swarm.dial(
                                DialOpts::peer_id(peer_id).addresses(vec![addr]).build(),
                            ) {
                                warn!("Failed to dial {peer_id}: {e}");
                            }
                        }
                    }
                    SwarmEvent::Behaviour(EnochEvent::Mdns(mdns::Event::Expired(peers))) => {
                        for (peer_id, _) in peers {
                            info!("mDNS expired: {peer_id}");
                        }
                    }
                    SwarmEvent::Behaviour(EnochEvent::Identify(identify::Event::Received {
                        peer_id, info, ..
                    })) => {
                        for addr in &info.listen_addrs {
                            swarm.behaviour_mut().kad.add_address(&peer_id, addr.clone());
                        }
                    }
                    SwarmEvent::Behaviour(EnochEvent::Ping(e)) => {
                        tracing::debug!("Ping: {e:?}");
                    }
                    SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                        warn!("Outgoing error to {peer_id:?}: {error}");
                    }
                    _ => {}
                }
            }
        } => {}
    }

    Ok(())
}
