//! Per-circle spawn logic — called at daemon startup, on hot-reload, and from the
//! `POST /circles/<id>/start` API endpoint.

use anyhow::Result;
use libp2p::{
    core::muxing::StreamMuxerBox,
    dcutr, futures::StreamExt,
    identify, kad, mdns, noise, pnet, quic, relay, rendezvous, tcp, yamux,
    swarm::{dial_opts::{DialOpts, PeerCondition}, SwarmEvent},
    Multiaddr, PeerId, SwarmBuilder,
};
use std::collections::HashSet;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use yrs::{Array, Observable};

use crate::{
    config::{self, CircleConfig, JoinPolicy},
    control::{MemberEntry, MemberRole, MLS_KEY_PACKAGES_KEY, MLS_OWNER_CLAIMS_KEY, MLS_PENDING_KEY, MLS_WELCOMES_KEY, MLS_COMMITS_KEY, MlsCommitEntry, OwnerClaim, PendingEntry, MEMBER_LIST_KEY},
    crypto::{keypair_from_hex, psk_from_hex},
    daemon::DaemonState,
    mls::{MlsGroupManager, MlsIdentity, SharedMlsState},
    network::{
        behaviour::{EnochBehaviour, EnochEvent},
        sync,
    },
    presence,
    state::AppState,
    sync_yjs::watcher::spawn_watcher,
};
use libp2p_stream as stream_proto;

/// Non-async shim with a concrete return type so that `rotate_psk_and_restart`
/// can call `spawn_circle` without creating an opaque-type inference cycle.
fn spawn_circle_boxed(
    config: CircleConfig,
    daemon: DaemonState,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
    Box::pin(spawn_circle(config, daemon))
}

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
    let cdir = config::circle_dir(&config.circle_id)?;
    let session_id = crate::store::session::next_session_id(&cdir).await;

    let mls_identity = MlsIdentity::load_or_generate(&cdir, &peer_id.to_string())?;
    let mls_group = MlsGroupManager::load(&mls_identity, &cdir)?;
    let mls = crate::mls::new_mls_state(mls_identity, mls_group);

    let state = AppState::new(
        config.circle_id.clone(),
        config.circle_name.clone(),
        workspace.clone(),
        cdir.clone(),
        config.admin_pubkey_hex.clone(),
        agent_id.clone(),
        session_id,
        peer_id.to_string(),
        config.join_policy.clone(),
        config.owner.clone(),
        mls.clone(),
    );

    let token = CancellationToken::new();

    // Publish MLS key package
    {
        use yrs::{Map, Transact};
        let kp_bytes = {
            let mls_locked = mls.blocking_lock();
            mls_locked.identity.generate_key_package()?
        };
        let kp_hex = hex::encode(&kp_bytes);
        let kp_map = state.control.get_or_insert_map(MLS_KEY_PACKAGES_KEY);
        let mut txn = state.control.transact_mut();
        kp_map.insert(&mut txn, peer_id.to_string().as_str(), kp_hex.as_str());
    }

    // Sign and publish owner claim
    {
        use yrs::{Map, Transact};
        let owner_claim_msg = format!("owner:{}", config.owner);
        let owner_sig = keypair.sign(owner_claim_msg.as_bytes()).map(hex::encode).unwrap_or_default();
        let claim = OwnerClaim { owner: config.owner.clone(), sig: owner_sig };
        if let Ok(json_str) = serde_json::to_string(&claim) {
            let claims_map = state.control.get_or_insert_map(MLS_OWNER_CLAIMS_KEY);
            let mut txn = state.control.transact_mut();
            claims_map.insert(&mut txn, peer_id.to_string().as_str(), json_str.as_str());
        }
    }

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
                owner: config.owner.clone(),
                agent_id: agent_id.clone(),
                role,
                added_at: chrono::Utc::now(),
                signature,
            };
            if let Ok(json_str) = serde_json::to_string(&entry) {
                let mut txn = state.control.transact_mut();
                map.insert(&mut txn, peer_id.to_string().as_str(), json_str.as_str());
            }

            // Write pending entry if not in member list
            let pending_entry = PendingEntry {
                peer_id: peer_id.to_string(),
                owner: config.owner.clone(),
                agent_id: agent_id.clone(),
                owner_sig: {
                    let owner_claim_msg = format!("owner:{}", config.owner);
                    keypair.sign(owner_claim_msg.as_bytes()).map(hex::encode).unwrap_or_default()
                },
                requested_at: chrono::Utc::now(),
            };
            if let Ok(json_str) = serde_json::to_string(&pending_entry) {
                let pending_map = state.control.get_or_insert_map(MLS_PENDING_KEY);
                let mut txn = state.control.transact_mut();
                pending_map.insert(&mut txn, peer_id.to_string().as_str(), json_str.as_str());
            }
        }
    }

    // If admin and auto join policy, observe pending map
    let is_admin = cdir.join("admin.key").exists();
    if is_admin && config.join_policy == JoinPolicy::Auto {
        let pending_map = state.control.get_or_insert_map(MLS_PENDING_KEY);
        let state_for_pending = state.clone();
        let mls_for_pending = mls.clone();
        let pending_sub = pending_map.observe(move |txn: &yrs::TransactionMut, event: &yrs::types::map::MapEvent| {
            let is_p2p = txn.origin().map(|o| o.as_ref() == b"p2p").unwrap_or(false);
            if !is_p2p { return; }
            for (key, change) in event.keys(txn) {
                if let yrs::types::EntryChange::Inserted(_) = change {
                    let peer_id_str = key.to_string();
                    let s = state_for_pending.clone();
                    let m = mls_for_pending.clone();
                    tokio::spawn(async move {
                        auto_approve(peer_id_str, s, m).await;
                    });
                }
            }
        });
        std::mem::forget(pending_sub);
    }

    // ── Welcome consumer ─────────────────────────────────────────────────────
    // Joiner: watch mls_welcomes for our own peer_id appearing (P2P-delivered).
    // Also handles the offline-approval case — initial full P2P sync fires the
    // observer for all pre-existing map entries.
    {
        let has_group = mls.blocking_lock().group.is_some();
        if !has_group {
            let welcomes_map = state.control.get_or_insert_map(MLS_WELCOMES_KEY);
            let our_peer_id_str = peer_id.to_string();
            let mls_w = mls.clone();
            let state_w = state.clone();
            let daemon_w = daemon.clone();
            let circle_id_w = config.circle_id.clone();
            let welcome_sub = welcomes_map.observe(move |txn: &yrs::TransactionMut, event: &yrs::types::map::MapEvent| {
                use yrs::types::EntryChange;
                for (key, change) in event.keys(txn) {
                    if key.as_ref() != our_peer_id_str.as_str() { continue; }
                    if let EntryChange::Inserted(yrs::Out::Any(yrs::Any::String(s))) = change {
                        let welcome_hex = s.to_string();
                        let mls = mls_w.clone();
                        let state = state_w.clone();
                        let daemon = daemon_w.clone();
                        let circle_id = circle_id_w.clone();
                        tokio::spawn(async move {
                            consume_welcome(welcome_hex, mls, state, daemon, circle_id).await;
                        });
                    }
                }
            });
            std::mem::forget(welcome_sub);
        }
    }

    // ── Commit watcher ────────────────────────────────────────────────────────
    // All peers: watch mls_commits for new entries and apply them to stay in
    // sync with epoch advances. PSK rotates after each epoch change.
    {
        use yrs::types::Change;
        let commits_arr = state.control.get_or_insert_array(MLS_COMMITS_KEY);
        let mls_c = mls.clone();
        let state_c = state.clone();
        let daemon_c = daemon.clone();
        let circle_id_c = config.circle_id.clone();
        let commits_sub = commits_arr.observe(move |txn: &yrs::TransactionMut, event: &yrs::types::array::ArrayEvent| {
            let is_p2p = txn.origin().map(|o| o.as_ref() == b"p2p").unwrap_or(false);
            if !is_p2p { return; }
            for change in event.delta(txn) {
                if let Change::Added(values) = change {
                    for val in values {
                        if let yrs::Out::Any(yrs::Any::String(s)) = val {
                            if let Ok(entry) = serde_json::from_str::<MlsCommitEntry>(&s) {
                                let mls = mls_c.clone();
                                let state = state_c.clone();
                                let daemon = daemon_c.clone();
                                let circle_id = circle_id_c.clone();
                                tokio::spawn(async move {
                                    apply_commit_entry(entry, mls, state, daemon, circle_id).await;
                                });
                            }
                        }
                    }
                }
            }
        });
        std::mem::forget(commits_sub);
    }

    spawn_watcher(state.clone(), workspace, token.clone()).await?;
    presence::spawn_presence(state.clone(), agent_id, token.clone());

    // ── Build the P2P swarm ───────────────────────────────────────────────────
    let pnet_config = pnet::PnetConfig::new(pnet::PreSharedKey::new(psk_bytes));
    let keypair_clone = keypair.clone();

    // relay::client::new produces the relay transport (for dialing circuits) and
    // the relay client behaviour (for managing reservations).
    let (relay_transport, relay_client_behaviour) = relay::client::new(peer_id);

    let mut swarm = SwarmBuilder::with_existing_identity(keypair.clone())
        .with_tokio()
        .with_other_transport(move |key| {
            use futures::future::Either;
            use libp2p::{core::upgrade, Transport};

            // TCP + PSK: used for LAN / direct connections within the circle.
            let tcp = tcp::tokio::Transport::new(tcp::Config::default())
                .and_then(move |s, _| pnet_config.handshake(s))
                .upgrade(upgrade::Version::V1Lazy)
                .authenticate(noise::Config::new(key)?)
                .multiplex(yamux::Config::default())
                .map(|(id, muxer), _| (id, StreamMuxerBox::new(muxer)));

            // Relay: used for circuit connections through a relay node.
            // No PSK here — relay connections are already over an authenticated channel.
            let relay = relay_transport
                .upgrade(upgrade::Version::V1Lazy)
                .authenticate(noise::Config::new(key)?)
                .multiplex(yamux::Config::default())
                .map(|(id, muxer), _| (id, StreamMuxerBox::new(muxer)));

            // QUIC: no PSK — used for bootstrap/rendezvous server connections.
            // Bootstrap servers don't share the circle PSK; they speak plain QUIC.
            let quic_t = quic::tokio::Transport::new(quic::Config::new(key))
                .map(|(id, muxer), _| (id, StreamMuxerBox::new(muxer)));

            Ok(tcp.or_transport(relay).or_transport(quic_t).map(|e, _| match e {
                Either::Left(Either::Left(x)) => x,
                Either::Left(Either::Right(x)) => x,
                Either::Right(x) => x,
            }))
        })?
        .with_behaviour(move |key| {
            let pid = key.public().to_peer_id();
            Ok(EnochBehaviour {
                mdns: mdns::tokio::Behaviour::new(mdns::Config::default(), pid)?,
                kad: {
                    let mut kad = kad::Behaviour::new(
                        pid,
                        kad::store::MemoryStore::new(pid),
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
                relay_client: relay_client_behaviour,
                relay: relay::Behaviour::new(pid, relay::Config::default()),
                dcutr: dcutr::Behaviour::new(pid),
                stream: stream_proto::Behaviour::new(),
            })
        })?
        .build();

    let p2p_addr: Multiaddr = "/ip4/0.0.0.0/tcp/0".parse().unwrap();
    swarm.listen_on(p2p_addr)?;

    // ── Dial bootstrap peers from config ──────────────────────────────────────
    // Peer addresses saved at `enoch enter` time (from invite). This ensures
    // connectivity even when mDNS is unavailable (different subnets, firewalls).
    for peer_str in &config.peers {
        match peer_str.parse::<Multiaddr>() {
            Ok(addr) => {
                info!("[{}] dialing bootstrap peer {addr}", config.circle_id);
                let _ = swarm.dial(addr);
            }
            Err(e) => warn!("[{}] invalid peer addr '{}': {e}", config.circle_id, peer_str),
        }
    }

    // ── Connect through relay nodes ───────────────────────────────────────────
    // Listening on a p2p-circuit address causes libp2p to connect to the relay
    // and request a reservation slot, making us reachable from any network.
    for relay_str in &config.relay_addrs {
        match relay_str.parse::<Multiaddr>() {
            Ok(relay_addr) => {
                let circuit_addr = relay_addr
                    .clone()
                    .with(libp2p::multiaddr::Protocol::P2pCircuit);
                info!("[{}] reserving relay slot at {relay_addr}", config.circle_id);
                if let Err(e) = swarm.listen_on(circuit_addr) {
                    warn!("[{}] relay circuit listen failed: {e}", config.circle_id);
                }
            }
            Err(e) => warn!("[{}] invalid relay addr '{}': {e}", config.circle_id, relay_str),
        }
    }

    // ── Dial rendezvous servers (QUIC) ────────────────────────────────────────
    // Rendezvous servers speak QUIC without PSK. After connecting we register
    // under the circle UUID namespace and discover other members.
    let rendezvous_peers: HashSet<PeerId> = config.rendezvous_addrs
        .iter()
        .filter_map(|s| {
            let addr: Multiaddr = s.parse().ok()?;
            addr.iter().find_map(|p| {
                if let libp2p::multiaddr::Protocol::P2p(id) = p { Some(id) } else { None }
            })
        })
        .collect();

    for rdvz_str in &config.rendezvous_addrs {
        match rdvz_str.parse::<Multiaddr>() {
            Ok(addr) => {
                info!("[{}] dialing rendezvous server {addr}", config.circle_id);
                let _ = swarm.dial(addr);
            }
            Err(e) => warn!("[{}] invalid rendezvous addr '{}': {e}", config.circle_id, rdvz_str),
        }
    }

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
    let rendezvous_namespace = rendezvous::Namespace::new(circle_id.clone())
        .unwrap_or_else(|_| rendezvous::Namespace::from_static("enochian"));

    tokio::spawn(async move {
        // Re-register with rendezvous servers every hour (TTL is 2h).
        let mut reregister = tokio::time::interval(std::time::Duration::from_secs(3600));
        reregister.tick().await; // skip the immediate first tick

        loop {
            tokio::select! {
                _ = swarm_token.cancelled() => {
                    info!("[{}] circle stopped", circle_id);
                    break;
                }
                _ = reregister.tick() => {
                    for &rdvz_peer in &rendezvous_peers {
                        if swarm.is_connected(&rdvz_peer) {
                            if let Err(e) = swarm.behaviour_mut().rendezvous.register(
                                rendezvous_namespace.clone(), rdvz_peer, None,
                            ) {
                                warn!("[{}] rendezvous re-register: {e}", circle_id);
                            }
                        }
                    }
                }
                event = swarm.select_next_some() => match event {
                    SwarmEvent::NewListenAddr { address, .. } => {
                        info!("[{}] P2P listening on {address}", circle_id);
                        // Track non-loopback, non-unspecified, non-circuit listen addrs.
                        // On a VPS these include the real public IP immediately at startup.
                        if is_routable_listen_addr(&address) {
                            if let Ok(mut addrs) = state_for_swarm.p2p_listen_addrs.write() {
                                let s = address.to_string();
                                if !addrs.contains(&s) { addrs.push(s); }
                            }
                        }
                    }
                    SwarmEvent::ExternalAddrConfirmed { address } => {
                        info!("[{}] external address confirmed: {address}", circle_id);
                        if let Ok(mut addrs) = state_for_swarm.p2p_external_addrs.write() {
                            let s = address.to_string();
                            if !addrs.contains(&s) { addrs.push(s); }
                        }
                    }
                    SwarmEvent::ExternalAddrExpired { address } => {
                        if let Ok(mut addrs) = state_for_swarm.p2p_external_addrs.write() {
                            addrs.retain(|a| a != &address.to_string());
                        }
                    }
                    SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                        info!("[{}] P2P connected: {peer_id} via {}", circle_id, endpoint.get_remote_address());
                        // If this is a rendezvous server, register + discover immediately.
                        if rendezvous_peers.contains(&peer_id) {
                            if let Err(e) = swarm.behaviour_mut().rendezvous.register(
                                rendezvous_namespace.clone(), peer_id, None,
                            ) {
                                warn!("[{}] rendezvous register at {peer_id}: {e}", circle_id);
                            }
                            swarm.behaviour_mut().rendezvous.discover(
                                Some(rendezvous_namespace.clone()), None, None, peer_id,
                            );
                        }
                        if endpoint.is_dialer() {
                            // Don't open sync stream to rendezvous-only servers.
                            if !rendezvous_peers.contains(&peer_id) {
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
                    SwarmEvent::Behaviour(EnochEvent::Rendezvous(e)) => {
                        use rendezvous::client::Event as RE;
                        match e {
                            RE::Registered { rendezvous_node, ttl, .. } => {
                                info!("[{}] rendezvous registered at {rendezvous_node} (ttl={ttl}s)", circle_id);
                            }
                            RE::RegisterFailed { rendezvous_node, error, .. } => {
                                warn!("[{}] rendezvous register failed at {rendezvous_node}: {error:?}", circle_id);
                            }
                            RE::Discovered { registrations, rendezvous_node, .. } => {
                                info!("[{}] rendezvous discovered {} peers from {rendezvous_node}", circle_id, registrations.len());
                                for reg in registrations {
                                    let pid = reg.record.peer_id();
                                    if pid == *swarm.local_peer_id() { continue; }
                                    for addr in reg.record.addresses() {
                                        swarm.behaviour_mut().kad.add_address(&pid, addr.clone());
                                        let _ = swarm.dial(
                                            DialOpts::peer_id(pid)
                                                .addresses(vec![addr.clone()])
                                                .condition(PeerCondition::DisconnectedAndNotDialing)
                                                .build(),
                                        );
                                    }
                                }
                            }
                            RE::DiscoverFailed { rendezvous_node, error, .. } => {
                                warn!("[{}] rendezvous discover failed at {rendezvous_node}: {error:?}", circle_id);
                            }
                            _ => {}
                        }
                    }
                    SwarmEvent::Behaviour(EnochEvent::RelayClient(e)) => {
                        tracing::debug!("[{}] relay client: {e:?}", circle_id);
                    }
                    SwarmEvent::Behaviour(EnochEvent::Dcutr(e)) => {
                        tracing::debug!("[{}] dcutr: {e:?}", circle_id);
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

async fn auto_approve(peer_id_str: String, state: AppState, mls: crate::mls::SharedMlsState) {
    use yrs::{Map, Out, Any, Transact};

    let kp_hex = {
        let kp_map = state.control.get_or_insert_map(MLS_KEY_PACKAGES_KEY);
        let txn = state.control.transact();
        match kp_map.get(&txn, peer_id_str.as_str()) {
            Some(Out::Any(Any::String(s))) => s.to_string(),
            _ => return,
        }
    };

    let kp_bytes = match hex::decode(&kp_hex) {
        Ok(b) => b,
        Err(_) => return,
    };

    let result = {
        let mut mls_locked = mls.lock().await;
        let identity_ptr = &mls_locked.identity as *const _;
        let group = match mls_locked.group.as_mut() {
            Some(g) => g,
            None => return,
        };
        let identity = unsafe { &*identity_ptr };
        group.add_member(identity, &kp_bytes)
    };

    let (commit_bytes, welcome_bytes, ratchet_tree_bytes) = match result {
        Ok(t) => t,
        Err(_) => return,
    };

    let welcome_hex = hex::encode(&welcome_bytes);
    let commit_hex = hex::encode(&commit_bytes);
    let ratchet_hex = hex::encode(&ratchet_tree_bytes);

    {
        let welcomes_map = state.control.get_or_insert_map(MLS_WELCOMES_KEY);
        let mut txn = state.control.transact_mut();
        welcomes_map.insert(&mut txn, peer_id_str.as_str(), welcome_hex.as_str());
    }

    {
        let commits_arr = state.control.get_or_insert_array(MLS_COMMITS_KEY);
        let mls_locked = mls.lock().await;
        let epoch = mls_locked.group.as_ref().map(|g| g.epoch()).unwrap_or(0);
        let entry = MlsCommitEntry {
            epoch,
            data_hex: commit_hex,
            sender_peer_id: state.peer_id.clone(),
            ratchet_tree_hex: ratchet_hex,
        };
        if let Ok(json_str) = serde_json::to_string(&entry) {
            let mut txn = state.control.transact_mut();
            commits_arr.push_back(&mut txn, json_str.as_str());
        }
    }

    {
        let member_map = state.control.get_or_insert_map(MEMBER_LIST_KEY);
        let pending_map = state.control.get_or_insert_map(MLS_PENDING_KEY);
        let pending_entry: Option<PendingEntry> = {
            let txn = state.control.transact();
            pending_map.get(&txn, peer_id_str.as_str()).and_then(|v| {
                if let Out::Any(Any::String(s)) = v {
                    serde_json::from_str(&s).ok()
                } else {
                    None
                }
            })
        };
        let (owner, agent_id) = pending_entry
            .map(|p| (p.owner, p.agent_id))
            .unwrap_or_default();
        let msg = format!("add:{}:member:owner:{}", peer_id_str, owner);
        let entry = MemberEntry {
            peer_id: peer_id_str.clone(),
            owner,
            agent_id,
            role: MemberRole::Member,
            added_at: chrono::Utc::now(),
            signature: msg,
        };
        if let Ok(json_str) = serde_json::to_string(&entry) {
            let mut txn = state.control.transact_mut();
            member_map.insert(&mut txn, peer_id_str.as_str(), json_str.as_str());
            pending_map.remove(&mut txn, peer_id_str.as_str());
        }
    }

    {
        let mls_locked = mls.lock().await;
        if let Some(group) = &mls_locked.group {
            let _ = group.save(&mls_locked.identity, &state.circle_dir);
        }
    }

    let _ = state.events.send(crate::control::CircleEvent::MemberAdded { peer_id: peer_id_str });
}

// ── PSK rotation helpers ──────────────────────────────────────────────────────

/// Called by the joiner when mls_welcomes[our_peer_id] arrives via P2P sync.
async fn consume_welcome(
    welcome_hex: String,
    mls: SharedMlsState,
    state: AppState,
    daemon: DaemonState,
    circle_id: String,
) {
    let welcome_bytes = match hex::decode(&welcome_hex) {
        Ok(b) => b,
        Err(_) => return,
    };

    let new_psk = {
        let mut mls_locked = mls.lock().await;
        // Skip if we already joined (race: observer fires twice).
        if mls_locked.group.is_some() { return; }
        let identity_ptr = &mls_locked.identity as *const MlsIdentity;
        let identity = unsafe { &*identity_ptr };
        // ratchet_tree_bytes is None because use_ratchet_tree_extension is enabled —
        // the ratchet tree is embedded inside the Welcome bytes.
        let group = match MlsGroupManager::join_from_welcome(identity, &welcome_bytes, None) {
            Ok(g) => g,
            Err(e) => { warn!("[mls] join_from_welcome failed: {e}"); return; }
        };
        let psk = match group.epoch_psk(identity) {
            Ok(p) => p,
            Err(e) => { warn!("[mls] epoch_psk after welcome failed: {e}"); return; }
        };
        let _ = group.save(identity, &state.circle_dir);
        mls_locked.group = Some(group);
        info!("[mls] joined group via Welcome — epoch PSK derived, rotating transport PSK");
        psk
    };

    rotate_psk_and_restart(&circle_id, new_psk, daemon).await;
}

/// Called for every new MlsCommitEntry that arrives from a peer.
/// Skips commits already applied (epoch < current), skips our own commits,
/// and does nothing if the group becomes inactive (we were removed — we'll
/// be locked out naturally when others rotate to the new PSK).
async fn apply_commit_entry(
    entry: MlsCommitEntry,
    mls: SharedMlsState,
    state: AppState,
    daemon: DaemonState,
    circle_id: String,
) {
    // Don't apply commits we ourselves produced.
    if entry.sender_peer_id == state.peer_id { return; }

    let commit_bytes = match hex::decode(&entry.data_hex) {
        Ok(b) => b,
        Err(_) => return,
    };

    let maybe_psk = {
        let mut mls_locked = mls.lock().await;
        // Take raw pointer to identity before the mutable group borrow.
        let identity_ptr = &mls_locked.identity as *const MlsIdentity;
        let identity = unsafe { &*identity_ptr };
        let group = match mls_locked.group.as_mut() {
            Some(g) => g,
            None => return, // not in group yet — will consume via Welcome path
        };
        let current_epoch = group.epoch();
        // Commit at epoch N advances us from epoch N → N+1.
        // Skip if we're already past this epoch.
        if entry.epoch < current_epoch { return; }

        match group.apply_commit(identity, &commit_bytes) {
            Ok(()) => {}
            Err(e) => { warn!("[mls] apply_commit (epoch {}): {e}", entry.epoch); return; }
        }

        // Derive new PSK. If this fails we were likely removed — skip rotation
        // and let the PSK mismatch handle disconnection naturally.
        match group.epoch_psk(identity) {
            Ok(psk) => {
                let _ = group.save(identity, &state.circle_dir);
                info!("[mls] applied Commit epoch {} → {} — rotating PSK", entry.epoch, group.epoch());
                Some(psk)
            }
            Err(_) => {
                info!("[mls] applied Commit that removed us — awaiting PSK mismatch disconnect");
                None
            }
        }
    };

    if let Some(new_psk) = maybe_psk {
        rotate_psk_and_restart(&circle_id, new_psk, daemon).await;
    }
}

/// Saves the new PSK to config.toml, then stops and restarts the circle so
/// the swarm rebuilds its transport with the new pnet key.
async fn rotate_psk_and_restart(circle_id: &str, new_psk: [u8; 32], daemon: DaemonState) {
    let mut cfg = match config::load(circle_id) {
        Ok(c) => c,
        Err(e) => { warn!("[mls] rotate_psk: config load failed: {e}"); return; }
    };
    cfg.psk_hex = hex::encode(new_psk);
    if let Err(e) = config::save(&cfg) {
        warn!("[mls] rotate_psk: config save failed: {e}");
        return;
    }
    info!("[mls] PSK rotated for circle {circle_id} — restarting swarm");

    let id = circle_id.to_string();
    tokio::spawn(async move {
        daemon.stop_circle(&id);
        // Brief pause so the old swarm tasks finish draining before we rebuild.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        match config::load(&id) {
            Ok(new_cfg) if !new_cfg.disabled => {
                // spawn_circle_boxed has a concrete return type, breaking the
                // opaque-type cycle between spawn_circle and rotate_psk_and_restart.
                if let Err(e) = spawn_circle_boxed(new_cfg, daemon).await {
                    warn!("[mls] rotate_psk: respawn failed: {e}");
                }
            }
            _ => {}
        }
    });
}

/// Returns true for listen addresses worth tracking for invite embedding:
/// rejects loopback, unspecified, link-local, and p2p-circuit relay addresses.
/// RFC1918 and Tailscale CGNAT addresses are kept — `enoch invite` sorts them
/// after public IPs so a public address is preferred when available.
fn is_routable_listen_addr(addr: &Multiaddr) -> bool {
    use libp2p::multiaddr::Protocol;

    if addr.to_string().contains("p2p-circuit") { return false; }

    for proto in addr.iter() {
        match proto {
            Protocol::Ip4(ip) => {
                if ip.is_loopback() || ip.is_unspecified() || ip.is_link_local() {
                    return false;
                }
            }
            Protocol::Ip6(ip) => {
                if ip.is_loopback() || ip.is_unspecified() { return false; }
            }
            _ => {}
        }
    }
    true
}
