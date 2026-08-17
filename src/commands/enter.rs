use anyhow::{Context, Result};
use libp2p::{
    core::muxing::StreamMuxerBox,
    dcutr,
    futures::StreamExt,
    identify, noise, pnet, quic, relay,
    swarm::{behaviour::toggle::Toggle, SwarmEvent},
    tcp, yamux, Multiaddr, SwarmBuilder,
};
use tracing::{info, warn};

use libp2p_stream as stream_proto;

use crate::{
    cli::EnterArgs,
    commands::rendezvous as rdvz,
    config::{self, circle_dir, resolve_workspace_dir, CircleConfig},
    crypto::{keypair_to_hex, psk_from_hex},
    identity::DeviceIdentity,
    invite,
    mls::MlsIdentity,
    network::behaviour::{EnochBehaviour, EnochEvent},
};

pub async fn run(args: EnterArgs, client: &reqwest::Client) -> Result<()> {
    // ── Step 1: Resolve credentials from invite URI or legacy flags ───────────
    let (
        circle_id,
        circle_name,
        psk_hex,
        peer_from_invite,
        relay_from_invite,
        rendezvous_from_invite,
        admin_pubkey_hex,
    ) = if args.target.starts_with("enoxian://") {
        let payload = invite::decode(&args.target)?;
        invite::check_expiry(&payload)?;

        let name = payload
            .circle_name
            .clone()
            .unwrap_or_else(|| payload.circle_id.clone());
        println!("✦ Joining circle: {name} ({})", payload.circle_id);

        let psk_hex = hex::encode(payload.psk_bytes);
        let admin_pubkey_hex = payload
            .admin_pubkey_bytes
            .as_deref()
            .map(hex::encode)
            .unwrap_or_default();
        (
            payload.circle_id,
            name,
            psk_hex,
            payload.peer_addr,
            payload.relay_addr,
            payload.rendezvous_addr,
            admin_pubkey_hex,
        )
    } else {
        let secret = args.secret.as_deref().context(
            "--secret is required when target is a Circle ID (or pass an enoxian:// invite)",
        )?;
        let circle_id = args.target.clone();
        (
            circle_id.clone(),
            circle_id.clone(),
            secret.to_string(),
            None,
            None,
            None,
            String::new(),
        )
    };

    let peer = args.peer.or(peer_from_invite);

    // ── Build bootstrap peer list ─────────────────────────────────────────────
    // Start with the direct peer address from the invite (public IP or confirmed
    // external addr). Relay circuit addresses must come from the inviter's
    // libp2p peer id; the admin key in the invite is not that identity.
    let bootstrap_peers: Vec<String> = peer
        .as_deref()
        .map(|p| vec![p.to_string()])
        .unwrap_or_default();

    // --rendezvous on CLI takes precedence over the invite-embedded address.
    // Short forms like "enox.suzent.com" are auto-resolved to full multiaddrs.
    // Computed before the membership check so the "already a member" refresh can
    // persist it too.
    let cli_rendezvous = match args.rendezvous {
        Some(ref s) => Some(
            rdvz::resolve(s, client)
                .await
                .with_context(|| format!("could not resolve rendezvous server '{s}'"))?,
        ),
        None => None,
    };
    let rendezvous_addrs = cli_rendezvous
        .or(rendezvous_from_invite)
        .into_iter()
        .collect::<Vec<_>>();

    // ── Step 2: Conflict detection ────────────────────────────────────────────
    let existing = config::load_all()?;

    let workspace_resolution =
        resolve_workspace_dir(&circle_name, &circle_id, &existing, args.dir)?;

    let (workspace_dir, warn_msg) = match workspace_resolution {
        None => {
            // Same UUID — already a member. Don't silently no-op: refresh the
            // credentials/bootstrap from this invite so a re-invite can repair a
            // rotated PSK or a stale peer address (the common "already joined but
            // can't connect" case). Keep our existing identity and workspace.
            if let Ok(mut existing_cfg) = config::load(&circle_id) {
                let psk_changed = existing_cfg.psk_hex != psk_hex;
                existing_cfg.psk_hex = psk_hex.clone();
                existing_cfg.peers = bootstrap_peers.clone();
                existing_cfg.relay_addrs = relay_from_invite.clone().into_iter().collect();
                existing_cfg.rendezvous_addrs = rendezvous_addrs.clone();
                config::save(&existing_cfg).context("failed to refresh circle config")?;
                println!(
                    "✦ Already a member of '{circle_name}' — refreshed credentials from invite."
                );
                if psk_changed {
                    println!("  PSK updated (the circle key had changed).");
                }
            } else {
                println!("✦ Already a member of '{circle_name}' — nothing to do.");
            }
            println!();
            println!(
                "  Restart the daemon to apply: enox update --dev --no-pull (or `enox service restart`)"
            );
            println!("  Then: enox --circle \"{circle_name}\" status");
            return Ok(());
        }
        Some(r) => r,
    };

    if let Some(ref msg) = warn_msg {
        println!("{msg}");
    }

    // ── Step 3: Derive per-circle keypair from stable device identity ─────────
    // Using the device identity gives a stable peer ID for this circle across
    // restarts and re-joins, so the admin never needs to re-add this device.
    let device = DeviceIdentity::load_or_generate(None)?;
    let keypair = device.derive_circle_keypair(&circle_id)?;
    let peer_id = keypair.public().to_peer_id();
    info!(
        "Entering circle {circle_id} as {peer_id} (device: {})",
        device.device_label
    );

    tokio::fs::create_dir_all(&workspace_dir)
        .await
        .with_context(|| format!("failed to create workspace {}", workspace_dir.display()))?;
    let workspace_dir = config::normalize_workspace_dir(&workspace_dir)?;
    if let Some(conflict) = config::workspace_conflict(&workspace_dir, &circle_id, &existing)? {
        anyhow::bail!(
            "workspace {} resolves to a directory already owned by circle '{}' ({})",
            workspace_dir.display(),
            conflict.circle_name,
            conflict.circle_id
        );
    }

    let circle_config = CircleConfig {
        circle_id: circle_id.clone(),
        circle_name: circle_name.clone(),
        psk_hex: psk_hex.clone(),
        keypair_proto_hex: keypair_to_hex(&keypair)?,
        workspace_dir: workspace_dir.to_string_lossy().into_owned(),
        admin_pubkey_hex,
        disabled: false,
        force_relay: false,
        peers: bootstrap_peers,
        relay_addrs: relay_from_invite.clone().into_iter().collect(),
        rendezvous_addrs,
        join_policy: crate::config::JoinPolicy::default(),
        owner: args.owner.unwrap_or_else(|| {
            crate::identity::read_identity_display()
                .map(|(label, handle)| handle.unwrap_or(label))
                .unwrap_or_default()
        }),
    };
    config::save(&circle_config).context("failed to save circle config")?;

    // ── Generate MLS identity (M11) ───────────────────────────────────────────
    // Joiner creates their identity now; admin will send a Welcome via the
    // control doc (mls_welcomes) after running `enox member add`.
    let cdir = circle_dir(&circle_id)?;
    let mls_identity = MlsIdentity::generate(&peer_id.to_string())?;
    mls_identity.save(&cdir)?;

    println!("  Workspace : {}", workspace_dir.display());
    println!("  Config    → ~/.enoxian/circles/{circle_id}/config.toml");

    // ── Step 4: Optionally verify connectivity to the invite peer ─────────────
    // Skipped when called from the daemon API (no_verify=true): the API spawns
    // the circle's P2P swarm which handles connectivity in the background.
    // Blocking the HTTP handler for up to 10s is undesirable and caused 500s.
    if args.no_verify {
        println!("  Config saved. Circle will connect when the daemon starts.");
        return Ok(());
    }

    let Some(peer_addr_str) = peer else {
        println!();
        println!("  No peer address in invite. Start the daemon to connect via mDNS:");
        println!("    enox start");
        println!("    enox --circle \"{circle_name}\" status");
        return Ok(());
    };

    let addr: Multiaddr = peer_addr_str
        .parse()
        .context("invalid peer multiaddr in invite")?;
    let mut public_relay_peer_ids =
        crate::network::public_relay_transport::relay_peer_ids_from_addrs(relay_from_invite.iter());
    if crate::network::public_relay_transport::is_relayed_addr(&addr) {
        let Some(peer_id) = crate::network::public_relay_transport::relay_peer_id(&addr) else {
            anyhow::bail!("relay circuit address is missing relay peer id");
        };
        public_relay_peer_ids.insert(peer_id);
    }
    let public_relay_peer_ids = std::sync::Arc::new(std::sync::RwLock::new(public_relay_peer_ids));

    let psk_bytes = psk_from_hex(&psk_hex)?;
    let pnet_config = pnet::PnetConfig::new(pnet::PreSharedKey::new(psk_bytes));
    let keypair_clone = keypair.clone();

    use libp2p::kad;
    let peer_id = keypair.public().to_peer_id();

    // relay::client::new returns a (transport, behaviour) pair linked by a channel.
    // Both MUST be included in the swarm — dropping the transport while keeping the
    // behaviour causes a panic when the behaviour is polled ("unreachable code").
    let (relay_transport, relay_client) = relay::client::new(peer_id);

    let mut swarm = SwarmBuilder::with_existing_identity(keypair.clone())
        .with_tokio()
        .with_other_transport(move |key| {
            use futures::future::Either;
            use libp2p::{core::upgrade, Transport};

            // Public TCP without pnet, restricted to the invite's relay server.
            let public_tcp = crate::network::public_relay_transport::PublicRelayTransport::new(
                tcp::tokio::Transport::new(tcp::Config::default()),
                public_relay_peer_ids.clone(),
            )
            .upgrade(upgrade::Version::V1Lazy)
            .authenticate(noise::Config::new(key)?)
            .multiplex(yamux::Config::default())
            .map(|(id, muxer), _| (id, StreamMuxerBox::new(muxer)));

            // TCP + PSK: for direct circle-peer connections.
            let tcp = tcp::tokio::Transport::new(tcp::Config::default())
                .and_then(move |s, _| pnet_config.handshake(s))
                .upgrade(upgrade::Version::V1Lazy)
                .authenticate(noise::Config::new(key)?)
                .multiplex(yamux::Config::default())
                .map(|(id, muxer), _| (id, StreamMuxerBox::new(muxer)));

            // Relay: for circuit addresses (invite peer_addr may be a relay circuit).
            let relay = relay_transport
                .upgrade(upgrade::Version::V1Lazy)
                .authenticate(noise::Config::new(key)?)
                .multiplex(yamux::Config::default())
                .map(|(id, muxer), _| (id, StreamMuxerBox::new(muxer)));

            // QUIC: invite peer_addr may also be a QUIC address.
            let quic_t = quic::tokio::Transport::new(quic::Config::new(key))
                .map(|(id, muxer), _| (id, StreamMuxerBox::new(muxer)));

            Ok(libp2p::dns::tokio::Transport::system(
                public_tcp
                    .or_transport(tcp)
                    .or_transport(relay)
                    .or_transport(quic_t)
                    .map(|e, _| match e {
                        Either::Left(Either::Left(Either::Left(x))) => x,
                        Either::Left(Either::Left(Either::Right(x))) => x,
                        Either::Left(Either::Right(x)) => x,
                        Either::Right(x) => x,
                    }),
            )?)
        })?
        .with_behaviour(|key| {
            let pid = key.public().to_peer_id();
            Ok(EnochBehaviour {
                // Disabled: `enter` dials the invite address directly, so it
                // needs no LAN discovery. See EnochBehaviour::mdns.
                mdns: Toggle::from(None::<libp2p::mdns::tokio::Behaviour>),
                kad: kad::Behaviour::new(pid, kad::store::MemoryStore::new(pid)),
                identify: identify::Behaviour::new(identify::Config::new(
                    "/enoxian/1.0.0".to_string(),
                    key.public(),
                )),
                ping: libp2p::ping::Behaviour::default(),
                rendezvous: libp2p::rendezvous::client::Behaviour::new(keypair_clone),
                relay_client,
                relay: relay::Behaviour::new(pid, relay::Config::default()),
                dcutr: Toggle::from(Some(dcutr::Behaviour::new(pid))),
                stream: stream_proto::Behaviour::new(),
            })
        })?
        .build();

    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;
    swarm
        .dial(addr.clone())
        .context("failed to initiate dial")?;
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
                        println!("  Start the daemon: enox start");
                        println!("  Then: enox --circle \"{circle_name}\" status");
                        return Ok(());
                    }
                    SwarmEvent::OutgoingConnectionError { error, .. } => {
                        warn!("Could not reach peer: {error}");
                        println!("  (Config saved — connect via mDNS when Enoxian starts)");
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
