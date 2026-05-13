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
    config,
    crypto::keypair_from_hex,
    daemon::DaemonState,
    network::behaviour::{EnochBehaviour, EnochEvent},
    state::AppState,
    sync_yjs::watcher::spawn_watcher,
};

pub async fn run(args: ServeArgs) -> Result<()> {
    let configs = config::load_all().context("failed to load circle configs")?;
    if configs.is_empty() {
        anyhow::bail!("no circles found — run `enoch init` to create one");
    }

    info!("Starting enochd — {} circle(s) found", configs.len());

    let daemon = DaemonState::new();

    for config in configs {
        let keypair = keypair_from_hex(&config.keypair_proto_hex)?;
        let peer_id = keypair.public().to_peer_id();
        let workspace = if config.workspace_dir.is_empty() {
            crate::config::circle_dir(&config.circle_id)?.join("files")
        } else {
            std::path::PathBuf::from(&config.workspace_dir)
        };
        tokio::fs::create_dir_all(&workspace).await?;

        info!(
            "  Circle '{}' ({}) — PeerID: {} — SyncDir: {}",
            config.circle_name,
            config.circle_id,
            peer_id,
            workspace.display()
        );

        let state = AppState::new(
            config.circle_id.clone(),
            config.circle_name.clone(),
            workspace.clone(),
        );

        spawn_watcher(state.clone(), workspace).await?;
        daemon.insert(config.circle_id.clone(), state);

        // Spawn a P2P swarm for this circle on a random port
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

        // Random P2P port per circle
        let p2p_addr: Multiaddr = "/ip4/0.0.0.0/tcp/0".parse().unwrap();
        swarm.listen_on(p2p_addr)?;

        let circle_id = config.circle_id.clone();
        tokio::spawn(async move {
            loop {
                match swarm.select_next_some().await {
                    SwarmEvent::NewListenAddr { address, .. } => {
                        info!("[{}] P2P listening on {address}", circle_id);
                    }
                    SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                        info!("[{}] P2P connected: {peer_id} via {}", circle_id, endpoint.get_remote_address());
                    }
                    SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                        info!("[{}] P2P disconnected: {peer_id}: {cause:?}", circle_id);
                    }
                    SwarmEvent::Behaviour(EnochEvent::Mdns(mdns::Event::Discovered(peers))) => {
                        for (peer_id, addr) in peers {
                            info!("[{}] mDNS discovered: {peer_id} @ {addr}", circle_id);
                            swarm.behaviour_mut().kad.add_address(&peer_id, addr.clone());
                            if swarm.is_connected(&peer_id) { continue; }
                            if let Err(e) = swarm.dial(
                                DialOpts::peer_id(peer_id).addresses(vec![addr]).build(),
                            ) {
                                warn!("[{}] Failed to dial {peer_id}: {e}", circle_id);
                            }
                        }
                    }
                    SwarmEvent::Behaviour(EnochEvent::Mdns(mdns::Event::Expired(peers))) => {
                        for (peer_id, _) in peers {
                            info!("[{}] mDNS expired: {peer_id}", circle_id);
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
                        tracing::debug!("[{}] Ping: {e:?}", circle_id);
                    }
                    SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                        warn!("[{}] Outgoing error to {peer_id:?}: {error}", circle_id);
                    }
                    _ => {}
                }
            }
        });
    }

    // ── Single HTTP/WS server for all circles ─────────────────────────────
    let app = api::router(daemon);
    let http_addr = SocketAddr::from(([0, 0, 0, 0], args.port));
    let listener = tokio::net::TcpListener::bind(http_addr).await
        .with_context(|| format!("failed to bind HTTP server on :{}", args.port))?;
    info!("HTTP/WS listening on :{}", args.port);

    axum::serve(listener, app).await.context("axum server error")?;
    Ok(())
}
