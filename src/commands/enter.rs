use anyhow::{Context, Result};
use libp2p::{
    futures::StreamExt,
    identify, noise, tcp, yamux,
    swarm::SwarmEvent,
    Multiaddr, SwarmBuilder,
};
use tracing::{info, warn};

use crate::{
    cli::EnterArgs,
    config::{self, CircleConfig},
    crypto::{generate_keypair, keypair_to_hex},
    invite,
    network::behaviour::{EnochBehaviour, EnochEvent},
};

pub async fn run(args: EnterArgs) -> Result<()> {
    // ── Step 1: Resolve credentials from invite URI or legacy flags ───────────
    let (circle_id, circle_name, psk_hex, peer_from_invite) = if args.target.starts_with("enochian://") {
        let payload = invite::decode(&args.target)?;
        invite::check_expiry(&payload)?;

        let name = payload.circle_name.clone().unwrap_or_else(|| payload.circle_id.clone());
        println!("✦ Joining circle: {name} ({})", payload.circle_id);

        let psk_hex = hex::encode(payload.psk_bytes);
        (payload.circle_id, name, psk_hex, payload.peer_addr)
    } else {
        let secret = args.secret.as_deref()
            .context("--secret is required when target is a Circle ID (or pass an enochian:// invite)")?;
        let circle_id = args.target.clone();
        (circle_id.clone(), circle_id.clone(), secret.to_string(), None)
    };

    // --peer flag overrides any peer embedded in the invite
    let peer = args.peer.or(peer_from_invite);

    // ── Step 2: Generate a fresh keypair for this node's identity ─────────────
    let keypair = generate_keypair();
    let peer_id = keypair.public().to_peer_id();
    info!("Entering circle {circle_id} as {peer_id}");

    // ── Step 3: Save config immediately — before any network operations ───────
    let circle_config = CircleConfig {
        circle_id:         circle_id.clone(),
        circle_name:       circle_name.clone(),
        psk_hex:           psk_hex.clone(),
        keypair_proto_hex: keypair_to_hex(&keypair)?,
    };
    config::save(&circle_config)
        .context("failed to save circle config")?;
    println!("  Saved → ~/.enochian/circles/{circle_id}/config.toml");

    // ── Step 4: Optionally verify connectivity to the invite peer ─────────────
    let Some(peer_addr_str) = peer else {
        // No peer address — config is saved, user should start enochd to connect via mDNS
        println!();
        println!("  No peer address in invite. Start the daemon to connect via mDNS:");
        println!("    enochd");
        println!("    enoch --circle \"{circle_name}\" status");
        return Ok(());
    };

    let addr: Multiaddr = peer_addr_str
        .parse()
        .context("invalid peer multiaddr in invite, expected e.g. /ip4/1.2.3.4/tcp/9091")?;

    // Minimal swarm — no mDNS, just a direct dial to confirm the peer is reachable
    use libp2p::kad;
    let mut swarm = SwarmBuilder::with_existing_identity(keypair.clone())
        .with_tokio()
        .with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default)?
        .with_behaviour(|key| {
            let peer_id = key.public().to_peer_id();
            Ok(EnochBehaviour {
                mdns: libp2p::mdns::tokio::Behaviour::new(
                    libp2p::mdns::Config { ttl: std::time::Duration::from_secs(1), ..Default::default() },
                    peer_id,
                )?,
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
    swarm.dial(addr.clone()).context("failed to initiate dial")?;
    info!("Dialing peer at {addr}");

    let timeout = tokio::time::sleep(std::time::Duration::from_secs(10));
    tokio::pin!(timeout);

    loop {
        tokio::select! {
            event = swarm.select_next_some() => {
                match event {
                    SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                        println!("  ✦ Verified peer {peer_id} via {}", endpoint.get_remote_address());
                        println!();
                        println!("  Start the daemon: enochd");
                        println!("  Then: enoch --circle \"{circle_name}\" status");
                        return Ok(());
                    }
                    SwarmEvent::OutgoingConnectionError { error, .. } => {
                        warn!("Could not reach peer: {error}");
                        println!("  (Config saved — connect via mDNS when enochd starts)");
                        return Ok(());
                    }
                    SwarmEvent::Behaviour(EnochEvent::Ping(_)) => {}
                    _ => {}
                }
            }
            _ = &mut timeout => {
                println!("  (Peer not reachable within 10s — config saved, connect via mDNS later)");
                return Ok(());
            }
        }
    }
}
