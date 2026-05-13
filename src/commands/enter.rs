use anyhow::{Context, Result};
use libp2p::{
    futures::StreamExt,
    identify, kad, mdns, noise, tcp, yamux,
    swarm::{dial_opts::DialOpts, SwarmEvent},
    Multiaddr, SwarmBuilder,
};
use tracing::{info, warn};

use crate::{
    cli::EnterArgs,
    crypto::generate_keypair,
    network::behaviour::{EnochBehaviour, EnochEvent},
};

pub async fn run(args: EnterArgs) -> Result<()> {
    // Always generate a fresh ephemeral keypair for `enter`.
    // `serve` owns the circle's persistent keypair/PeerID.
    // Two nodes sharing the same PeerID cannot connect to each other.
    let keypair = generate_keypair();

    let peer_id = keypair.public().to_peer_id();
    info!("Entering circle {} as {peer_id}", args.circle_id);

    let mut swarm = SwarmBuilder::with_existing_identity(keypair.clone())
        .with_tokio()
        .with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default)?
        .with_behaviour(|key| {
            let peer_id = key.public().to_peer_id();
            Ok(EnochBehaviour {
                mdns: mdns::tokio::Behaviour::new(mdns::Config::default(), peer_id)?,
                kad: kad::Behaviour::new(peer_id, kad::store::MemoryStore::new(peer_id)),
                identify: identify::Behaviour::new(identify::Config::new(
                    "/enochian/1.0.0".to_string(),
                    key.public(),
                )),
                ping: libp2p::ping::Behaviour::default(),
                rendezvous: libp2p::rendezvous::client::Behaviour::new(keypair.clone()),
            })
        })?
        .build();

    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

    // Direct dial — bypasses mDNS (useful when multicast is blocked, e.g. Windows Firewall)
    if let Some(peer_addr) = &args.peer {
        let addr: Multiaddr = peer_addr
            .parse()
            .context("invalid peer multiaddr, expected e.g. /ip4/192.168.1.10/tcp/9090")?;
        info!("Dialing peer directly at {addr}");
        swarm.dial(addr)?;
    }

    if let Some(rendezvous_addr) = &args.rendezvous {
        let addr: Multiaddr = rendezvous_addr
            .parse()
            .context("invalid rendezvous multiaddr")?;
        info!("Dialing rendezvous server at {addr}");
        swarm.dial(addr)?;
    }

    loop {
        match swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { address, .. } => {
                info!("Listening on {address}");
            }
            SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                info!("✦ Connected to {peer_id} via {}", endpoint.get_remote_address());
            }
            SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                info!("Disconnected from {peer_id}: {cause:?}");
            }
            SwarmEvent::Behaviour(EnochEvent::Mdns(mdns::Event::Discovered(peers))) => {
                for (peer_id, addr) in peers {
                    info!("✦ mDNS discovered: {peer_id} @ {addr}");
                    swarm.behaviour_mut().kad.add_address(&peer_id, addr.clone());
                    // Skip dial if already connected — mDNS reports all addresses
                    // including duplicates; the "already connected" error is benign.
                    if swarm.is_connected(&peer_id) {
                        continue;
                    }
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
                peer_id,
                info,
                ..
            })) => {
                info!("Identified {peer_id}: {}", info.agent_version);
                for addr in &info.listen_addrs {
                    swarm.behaviour_mut().kad.add_address(&peer_id, addr.clone());
                }
            }
            SwarmEvent::Behaviour(EnochEvent::Ping(e)) => {
                tracing::debug!("Ping: {e:?}");
            }
            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                warn!("Connection error to {peer_id:?}: {error}");
            }
            _ => {}
        }
    }
}
