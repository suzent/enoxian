use anyhow::{Context, Result};
use libp2p::{
    futures::StreamExt,
    identify, kad, mdns, noise, tcp, yamux,
    swarm::{dial_opts::DialOpts, SwarmEvent},
    Multiaddr, SwarmBuilder,
};
use tracing::{info, warn};

use crate::{
    cli::ServeArgs,
    config::load,
    crypto::keypair_from_hex,
    network::behaviour::{EnochBehaviour, EnochEvent},
};

pub async fn run(args: ServeArgs) -> Result<()> {
    let config = load(&args.circle).context("circle not found — run `enochd init` first")?;
    let keypair = keypair_from_hex(&config.keypair_proto_hex)?;
    let peer_id = keypair.public().to_peer_id();

    info!("Starting enochd for circle '{}' ({})", config.circle_name, config.circle_id);
    info!("PeerID: {peer_id}");

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

    let listen_addr: Multiaddr = format!("/ip4/0.0.0.0/tcp/{}", args.port)
        .parse()
        .context("invalid listen addr")?;
    swarm.listen_on(listen_addr.clone())?;

    info!("Listening on {listen_addr}");

    loop {
        match swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { address, .. } => {
                info!("Listening on {address}");
            }
            SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                info!("Connected to {peer_id} via {}", endpoint.get_remote_address());
            }
            SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                info!("Disconnected from {peer_id}: {cause:?}");
            }
            SwarmEvent::Behaviour(EnochEvent::Mdns(mdns::Event::Discovered(peers))) => {
                for (peer_id, addr) in peers {
                    info!("mDNS discovered: {peer_id} @ {addr}");
                    swarm.behaviour_mut().kad.add_address(&peer_id, addr.clone());
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
                for addr in &info.listen_addrs {
                    swarm.behaviour_mut().kad.add_address(&peer_id, addr.clone());
                }
            }
            SwarmEvent::Behaviour(EnochEvent::Kad(e)) => {
                tracing::debug!("Kad: {e:?}");
            }
            SwarmEvent::Behaviour(EnochEvent::Ping(e)) => {
                tracing::debug!("Ping: {e:?}");
            }
            SwarmEvent::Behaviour(EnochEvent::Rendezvous(e)) => {
                tracing::debug!("Rendezvous: {e:?}");
            }
            SwarmEvent::IncomingConnection { local_addr, send_back_addr, .. } => {
                info!("Incoming connection from {send_back_addr} on {local_addr}");
            }
            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                warn!("Outgoing connection error to {peer_id:?}: {error}");
            }
            _ => {}
        }
    }
}
