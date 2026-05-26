use anyhow::{Context, Result};
use libp2p::{
    dcutr, futures::StreamExt,
    identify, noise, pnet, relay, tcp, yamux,
    swarm::SwarmEvent,
    Multiaddr, SwarmBuilder,
};
use tracing::{info, warn};

use libp2p_stream as stream_proto;

use crate::{
    cli::EnterArgs,
    commands::rendezvous as rdvz,
    config::{self, circle_dir, CircleConfig, resolve_workspace_dir},
    crypto::{generate_keypair, keypair_to_hex, psk_from_hex},
    invite,
    mls::MlsIdentity,
    network::behaviour::{EnochBehaviour, EnochEvent},
};

pub async fn run(args: EnterArgs, client: &reqwest::Client) -> Result<()> {
    // ── Step 1: Resolve credentials from invite URI or legacy flags ───────────
    let (circle_id, circle_name, psk_hex, peer_from_invite, relay_from_invite, rendezvous_from_invite, admin_pubkey_hex) = if args.target.starts_with("enochian://") {
        let payload = invite::decode(&args.target)?;
        invite::check_expiry(&payload)?;

        let name = payload.circle_name.clone().unwrap_or_else(|| payload.circle_id.clone());
        println!("✦ Joining circle: {name} ({})", payload.circle_id);

        let psk_hex = hex::encode(payload.psk_bytes);
        let admin_pubkey_hex = payload.admin_pubkey_bytes
            .as_deref()
            .map(hex::encode)
            .unwrap_or_default();
        (payload.circle_id, name, psk_hex, payload.peer_addr, payload.relay_addr, payload.rendezvous_addr, admin_pubkey_hex)
    } else {
        let secret = args.secret.as_deref()
            .context("--secret is required when target is a Circle ID (or pass an enochian:// invite)")?;
        let circle_id = args.target.clone();
        (circle_id.clone(), circle_id.clone(), secret.to_string(), None, None, None, String::new())
    };

    let peer = args.peer.or(peer_from_invite);

    // ── Step 2: Conflict detection ────────────────────────────────────────────
    let existing = config::load_all()?;

    let workspace_resolution = resolve_workspace_dir(
        &circle_name,
        &circle_id,
        &existing,
        args.dir,
    )?;

    let (workspace_dir, warn_msg) = match workspace_resolution {
        None => {
            // Same UUID — already a member
            println!("✦ Already a member of '{circle_name}' — nothing to do.");
            println!();
            println!("  Start the daemon: enochd");
            println!("  Then: enoch --circle \"{circle_name}\" status");
            return Ok(());
        }
        Some(r) => r,
    };

    if let Some(ref msg) = warn_msg {
        println!("{msg}");
    }

    // ── Step 3: Generate keypair + save config ────────────────────────────────
    let keypair = generate_keypair();
    let peer_id = keypair.public().to_peer_id();
    info!("Entering circle {circle_id} as {peer_id}");

    tokio::fs::create_dir_all(&workspace_dir).await
        .with_context(|| format!("failed to create workspace {}", workspace_dir.display()))?;

    // --rendezvous on CLI takes precedence over the invite-embedded address.
    // Short forms like "enoch.suzent.com" are auto-resolved to full multiaddrs.
    let cli_rendezvous = match args.rendezvous {
        Some(ref s) => Some(rdvz::resolve(s, client).await
            .with_context(|| format!("could not resolve rendezvous server '{s}'"))?),
        None => None,
    };
    let rendezvous_addrs = cli_rendezvous
        .or(rendezvous_from_invite)
        .into_iter()
        .collect::<Vec<_>>();

    // ── Build bootstrap peer list ─────────────────────────────────────────────
    // Start with the direct peer address from the invite (public IP or confirmed
    // external addr). Also derive the inviter's relay circuit address from the
    // relay addr + inviter peer ID (extracted from admin_pubkey_bytes) — this gives
    // a WAN-reachable bootstrap address that survives rendezvous being offline.
    let mut bootstrap_peers: Vec<String> =
        peer.as_deref().map(|p| vec![p.to_string()]).unwrap_or_default();

    if let (Some(ref relay_str), false) = (&relay_from_invite, admin_pubkey_hex.is_empty()) {
        // admin_pubkey_hex is hex-encoded protobuf of the inviter's public key.
        // Use it to derive their peer ID and construct their relay circuit address.
        if let Ok(pubkey_bytes) = hex::decode(&admin_pubkey_hex) {
            if let Ok(pubkey) = libp2p::identity::PublicKey::try_decode_protobuf(&pubkey_bytes) {
                let inviter_peer_id = pubkey.to_peer_id();
                if let Ok(relay_addr) = relay_str.parse::<Multiaddr>() {
                    let circuit = relay_addr
                        .with(libp2p::multiaddr::Protocol::P2pCircuit)
                        .with(libp2p::multiaddr::Protocol::P2p(inviter_peer_id));
                    let circuit_str = circuit.to_string();
                    // Don't duplicate if peer_addr was already the circuit address.
                    if !bootstrap_peers.contains(&circuit_str) {
                        bootstrap_peers.push(circuit_str);
                    }
                }
            }
        }
    }

    let circle_config = CircleConfig {
        circle_id:         circle_id.clone(),
        circle_name:       circle_name.clone(),
        psk_hex:           psk_hex.clone(),
        keypair_proto_hex: keypair_to_hex(&keypair)?,
        workspace_dir:     workspace_dir.to_string_lossy().into_owned(),
        admin_pubkey_hex,
        disabled:          false,
        peers:             bootstrap_peers,
        relay_addrs:       relay_from_invite.into_iter().collect(),
        rendezvous_addrs,
        join_policy:       crate::config::JoinPolicy::default(),
        owner:             args.owner.unwrap_or_default(),
    };
    config::save(&circle_config).context("failed to save circle config")?;

    // ── Generate MLS identity (M11) ───────────────────────────────────────────
    // Joiner creates their identity now; admin will send a Welcome via the
    // control doc (mls_welcomes) after running `enoch member add`.
    let cdir = circle_dir(&circle_id)?;
    let mls_identity = MlsIdentity::generate(&peer_id.to_string())?;
    mls_identity.save(&cdir)?;

    println!("  Workspace : {}", workspace_dir.display());
    println!("  Config    → ~/.enochian/circles/{circle_id}/config.toml");

    // ── Step 4: Optionally verify connectivity to the invite peer ─────────────
    let Some(peer_addr_str) = peer else {
        println!();
        println!("  No peer address in invite. Start the daemon to connect via mDNS:");
        println!("    enochd");
        println!("    enoch --circle \"{circle_name}\" status");
        return Ok(());
    };

    let addr: Multiaddr = peer_addr_str
        .parse()
        .context("invalid peer multiaddr in invite")?;

    let psk_bytes = psk_from_hex(&psk_hex)?;
    let pnet_config = pnet::PnetConfig::new(pnet::PreSharedKey::new(psk_bytes));
    let keypair_clone = keypair.clone();

    use libp2p::kad;
    let peer_id = keypair.public().to_peer_id();
    let (_, relay_client) = relay::client::new(peer_id);

    let mut swarm = SwarmBuilder::with_existing_identity(keypair.clone())
        .with_tokio()
        .with_other_transport(|key| {
            use libp2p::{core::{muxing::StreamMuxerBox, upgrade}, Transport};
            let noise = noise::Config::new(key)?;
            let transport = tcp::tokio::Transport::new(tcp::Config::default())
                .and_then(move |s, _| pnet_config.handshake(s))
                .upgrade(upgrade::Version::V1Lazy)
                .authenticate(noise)
                .multiplex(yamux::Config::default())
                .map(|(id, muxer), _| (id, StreamMuxerBox::new(muxer)));
            Ok(transport)
        })?
        .with_behaviour(|key| {
            let pid = key.public().to_peer_id();
            Ok(EnochBehaviour {
                mdns: libp2p::mdns::tokio::Behaviour::new(
                    libp2p::mdns::Config { ttl: std::time::Duration::from_secs(1), ..Default::default() },
                    pid,
                )?,
                kad: kad::Behaviour::new(pid, kad::store::MemoryStore::new(pid)),
                identify: identify::Behaviour::new(identify::Config::new(
                    "/enochian/1.0.0".to_string(),
                    key.public(),
                )),
                ping: libp2p::ping::Behaviour::default(),
                rendezvous: libp2p::rendezvous::client::Behaviour::new(keypair_clone),
                relay_client,
                relay: relay::Behaviour::new(pid, relay::Config::default()),
                dcutr: dcutr::Behaviour::new(pid),
                stream: stream_proto::Behaviour::new(),
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
