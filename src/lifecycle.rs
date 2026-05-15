//! Per-circle spawn logic — called at daemon startup, on hot-reload, and from the
//! `POST /circles/<id>/start` API endpoint.

use anyhow::Result;
use libp2p::{
    futures::StreamExt,
    identify, kad, mdns, noise, pnet, tcp, yamux,
    swarm::{dial_opts::DialOpts, SwarmEvent},
    Multiaddr, SwarmBuilder,
};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::{
    config::{self, CircleConfig},
    control::{MemberEntry, MemberRole, MEMBER_LIST_KEY},
    crypto::{keypair_from_hex, psk_from_hex},
    daemon::DaemonState,
    network::{
        behaviour::{EnochBehaviour, EnochEvent},
        sync,
    },
    presence,
    state::AppState,
    sync_yjs::watcher::spawn_watcher,
};
use libp2p_stream as stream_proto;

pub async fn spawn_circle(config: CircleConfig, daemon: DaemonState) -> Result<()> {
    let keypair = keypair_from_hex(&config.keypair_proto_hex)?;
    let peer_id = keypair.public().to_peer_id();
    let psk_bytes = psk_from_hex(&config.psk_hex)?;

    let workspace = if config.workspace_dir.is_empty() {
        crate::config::circle_dir(&config.circle_id)?.join("files")
    } else {
        std::path::PathBuf::from(&config.workspace_dir)
    };
    tokio::fs::create_dir_all(&workspace).await?;

    info!(
        "  Circle '{}' ({}) — PeerID: {} — Workspace: {}",
        config.circle_name, config.circle_id, peer_id, workspace.display()
    );

    let agent_id = presence::local_agent_id(&peer_id);
    let circle_dir = config::circle_dir(&config.circle_id)?;
    let session_id = crate::store::session::next_session_id(&circle_dir).await;
    let state = AppState::new(
        config.circle_id.clone(),
        config.circle_name.clone(),
        workspace.clone(),
        circle_dir,
        config.admin_pubkey_hex.clone(),
        agent_id.clone(),
        session_id,
    );

    let token = CancellationToken::new();

    // Auto-register local peer in the member list so `enoch member list` shows all participants.
    // Only writes if no entry exists yet — preserves explicit removals across restarts.
    {
        use yrs::{Map, Out, Any, Transact};
        let map = state.control.get_or_insert_map(MEMBER_LIST_KEY);
        let already_registered = {
            let txn = state.control.transact();
            matches!(map.get(&txn, peer_id.to_string().as_str()), Some(Out::Any(Any::String(_))))
        };
        if !already_registered {
            let role = config::circle_dir(&config.circle_id)
                .ok()
                .map(|d| if d.join("admin.key").exists() { MemberRole::Admin } else { MemberRole::Member })
                .unwrap_or(MemberRole::Member);
            let msg = format!("add:{}:{}", peer_id, role);
            let signature = keypair.sign(msg.as_bytes()).map(hex::encode).unwrap_or_default();
            let entry = MemberEntry {
                peer_id: peer_id.to_string(),
                agent_id: agent_id.clone(),
                role,
                added_at: chrono::Utc::now(),
                signature,
            };
            if let Ok(json_str) = serde_json::to_string(&entry) {
                let mut txn = state.control.transact_mut();
                map.insert(&mut txn, peer_id.to_string().as_str(), json_str.as_str());
            }
        }
    }

    spawn_watcher(state.clone(), workspace, token.clone()).await?;
    presence::spawn_presence(state.clone(), agent_id, token.clone());

    // ── Build the P2P swarm ───────────────────────────────────────────────────
    let pnet_config = pnet::PnetConfig::new(pnet::PreSharedKey::new(psk_bytes));
    let keypair_clone = keypair.clone();

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
                rendezvous: libp2p::rendezvous::client::Behaviour::new(keypair_clone),
                stream: stream_proto::Behaviour::new(),
            })
        })?
        .build();

    let p2p_addr: Multiaddr = "/ip4/0.0.0.0/tcp/0".parse().unwrap();
    swarm.listen_on(p2p_addr)?;

    // ── Accept incoming sync streams ──────────────────────────────────────────
    let mut stream_control = swarm.behaviour().stream.new_control();
    let state_for_accept = state.clone();
    let accept_token = token.clone();
    tokio::spawn(async move {
        let mut incoming = match stream_control.accept(sync::PROTOCOL) {
            Ok(s) => s,
            Err(e) => { warn!("[stream] accept failed: {e}"); return; }
        };
        loop {
            tokio::select! {
                _ = accept_token.cancelled() => break,
                item = incoming.next() => match item {
                    Some((peer_id, stream)) => {
                        let s = state_for_accept.clone();
                        tokio::spawn(sync::run_sync(peer_id, stream, s, false));
                    }
                    None => break,
                }
            }
        }
    });

    // ── Swarm event loop ──────────────────────────────────────────────────────
    let circle_id = config.circle_id.clone();
    let open_ctrl = swarm.behaviour().stream.new_control();
    let swarm_token = token.clone();
    let state_for_swarm = state.clone();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = swarm_token.cancelled() => {
                    info!("[{}] circle stopped", circle_id);
                    break;
                }
                event = swarm.select_next_some() => match event {
                    SwarmEvent::NewListenAddr { address, .. } => {
                        info!("[{}] P2P listening on {address}", circle_id);
                    }
                    SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                        info!("[{}] P2P connected: {peer_id} via {}", circle_id, endpoint.get_remote_address());
                        if endpoint.is_dialer() {
                            let mut ctrl = open_ctrl.clone();
                            let s = state_for_swarm.clone();
                            tokio::spawn(async move {
                                match ctrl.open_stream(peer_id, sync::PROTOCOL).await {
                                    Ok(stream) => sync::run_sync(peer_id, stream, s, true).await,
                                    Err(e) => warn!("[sync] open_stream to {peer_id}: {e}"),
                                }
                            });
                        }
                    }
                    SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                        info!("[{}] P2P disconnected: {peer_id}: {cause:?}", circle_id);
                    }
                    SwarmEvent::Behaviour(EnochEvent::Mdns(mdns::Event::Discovered(peers))) => {
                        for (peer_id, addr) in peers {
                            info!("[{}] mDNS discovered: {peer_id} @ {addr}", circle_id);
                            swarm.behaviour_mut().kad.add_address(&peer_id, addr.clone());
                            if let Err(e) = swarm.dial(
                                DialOpts::peer_id(peer_id)
                                    .addresses(vec![addr])
                                    .condition(libp2p::swarm::dial_opts::PeerCondition::DisconnectedAndNotDialing)
                                    .build(),
                            ) {
                                tracing::debug!("[{}] dial skipped: {e}", circle_id);
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
                        tracing::debug!("[{}] Outgoing error to {peer_id:?}: {error}", circle_id);
                    }
                    _ => {}
                }
            }
        }
    });

    daemon.insert_circle(config.circle_id.clone(), state, token);
    Ok(())
}
