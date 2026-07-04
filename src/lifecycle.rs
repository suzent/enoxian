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
        proposal_sync, sync,
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
            let mls_locked = mls.lock().await;
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

    // Auto-register local peer in the member list so `enox member list` shows all participants.
    // Only writes if no entry exists yet — preserves explicit removals across restarts.
    {
        use yrs::{Map, Out, Any, Transact};
        let map = state.control.get_or_insert_map(MEMBER_LIST_KEY);
        let already_registered = {
            let txn = state.control.transact();
            matches!(map.get(&txn, peer_id.to_string().as_str()), Some(Out::Any(Any::String(_))))
        };
        if !already_registered {
            let is_local_admin = cdir.join("admin.key").exists();
            let role = if is_local_admin { MemberRole::Admin } else { MemberRole::Member };
            let msg = format!("add:{}:{}", peer_id, role);
            let signature = keypair.sign(msg.as_bytes()).map(hex::encode).unwrap_or_default();
            let device_label = crate::identity::read_identity_display()
                .map(|(label, _)| label)
                .unwrap_or_default();
            let agents = crate::identity::read_local_agents();
            let entry = MemberEntry {
                peer_id: peer_id.to_string(),
                owner: config.owner.clone(),
                agent_id: agent_id.clone(),
                device_label,
                agents,
                role,
                added_at: chrono::Utc::now(),
                signature,
            };

            // Before inserting, evict any stale entries for this same device (same
            // agent_id, different peer_id). This happens when the user leaves and
            // rejoins: a new keypair is generated each time, leaving ghost entries.
            {
                use yrs::Out;
                let stale_keys: Vec<String> = {
                    let txn = state.control.transact();
                    map.iter(&txn)
                        .filter_map(|(key, val)| {
                            if key == peer_id.to_string().as_str() { return None; }
                            if let Out::Any(yrs::Any::String(s)) = val {
                                if let Ok(m) = serde_json::from_str::<MemberEntry>(&s) {
                                    if m.agent_id == agent_id {
                                        return Some(key.to_string());
                                    }
                                }
                            }
                            None
                        })
                        .collect()
                };
                if !stale_keys.is_empty() {
                    let mut txn = state.control.transact_mut();
                    for key in &stale_keys {
                        map.remove(&mut txn, key.as_str());
                    }
                    // Also remove their pending entries
                    let pending_map = state.control.get_or_insert_map(MLS_PENDING_KEY);
                    let mut txn = state.control.transact_mut();
                    for key in &stale_keys {
                        pending_map.remove(&mut txn, key.as_str());
                    }
                    info!("[member] evicted {} stale entr(ies) for device '{agent_id}' (device rejoined)", stale_keys.len());
                }
            }

            if let Ok(json_str) = serde_json::to_string(&entry) {
                let mut txn = state.control.transact_mut();
                map.insert(&mut txn, peer_id.to_string().as_str(), json_str.as_str());
            }

            // Admins never queue themselves as pending — they bootstrap the MLS group.
            // Non-admins write a pending entry so the admin can issue a Welcome.
            if !is_local_admin {
                let pending_entry = PendingEntry {
                    peer_id: peer_id.to_string(),
                    owner: config.owner.clone(),
                    agent_id: agent_id.clone(),
                    device_label: crate::identity::read_identity_display()
                        .map(|(label, _)| label)
                        .unwrap_or_default(),
                    agents: crate::identity::read_local_agents(),
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
        } else {
            // Already registered in member list — clean up any stale pending entry
            // that may have persisted from the first join (written before approval).
            // This handles the restart case: CRDT retained the old pending entry even
            // though we're already a member.
            use yrs::Out;
            let pending_map = state.control.get_or_insert_map(MLS_PENDING_KEY);
            let self_key = peer_id.to_string();
            {
                let txn = state.control.transact();
                if matches!(pending_map.get(&txn, self_key.as_str()), Some(Out::Any(_))) {
                    drop(txn);
                    let mut txn = state.control.transact_mut();
                    pending_map.remove(&mut txn, self_key.as_str());
                    info!("[member] removed stale pending entry for self (already a member)");
                }
            }
        }

        // If admin has no MLS group (e.g. circle predates M11 or group.json was lost),
        // bootstrap it now — admin is always leaf 0.
        let is_local_admin = cdir.join("admin.key").exists();
        if is_local_admin {
            let mut mls_locked = mls.lock().await;
            if mls_locked.group.is_none() {
                match MlsGroupManager::create(&mls_locked.identity) {
                    Ok(group) => {
                        if let Err(e) = group.save(&mls_locked.identity, &cdir) {
                            warn!("[mls] auto-bootstrap: failed to save group: {e}");
                        } else {
                            info!("[mls] auto-bootstrapped MLS group for pre-M11 circle");
                        }
                        mls_locked.group = Some(group);
                    }
                    Err(e) => warn!("[mls] auto-bootstrap: failed to create group: {e}"),
                }
            }
        }
    }

    // Admin: remove any stale pending entry for ourselves.
    //
    // There are TWO moments a stale entry can appear:
    //   1. It is already in the local Yjs doc at startup (e.g. written by an old
    //      binary before this guard existed). → Caught by the synchronous check below.
    //   2. It arrives via P2P sync AFTER startup (the remote peer's CRDT contains the
    //      entry and it replicates to us). → Caught by the observer below.
    //
    // Both cases must be handled; the observer alone misses case 1 because observers
    // only fire for new mutations, not for state already present at observe() time.
    let is_admin = cdir.join("admin.key").exists();
    if is_admin {
        use yrs::{Map, Transact};
        let pending_map = state.control.get_or_insert_map(MLS_PENDING_KEY);
        let self_peer_str = peer_id.to_string();

        // Case 1: already present locally.
        {
            use yrs::Out;
            let txn = state.control.transact();
            if matches!(pending_map.get(&txn, self_peer_str.as_str()), Some(Out::Any(_))) {
                drop(txn);
                let mut txn = state.control.transact_mut();
                pending_map.remove(&mut txn, self_peer_str.as_str());
            }
        }

        // Case 2: arrives later via P2P sync. Observe and evict immediately.
        let state_for_self_evict = state.clone();
        let self_evict_sub = pending_map.observe(move |txn: &yrs::TransactionMut, event: &yrs::types::map::MapEvent| {
            let is_p2p = txn.origin().map(|o| o.as_ref() == b"p2p").unwrap_or(false);
            if !is_p2p { return; }
            for (key, change) in event.keys(txn) {
                if key.as_ref() != self_peer_str.as_str() { continue; }
                if let yrs::types::EntryChange::Inserted(_) = change {
                    // Our own peer ID was just inserted by a remote — remove it.
                    let s = state_for_self_evict.clone();
                    let peer_str = self_peer_str.clone();
                    tokio::spawn(async move {
                        use yrs::{Map, Transact};
                        let pm = s.control.get_or_insert_map(MLS_PENDING_KEY);
                        let mut txn = s.control.transact_mut();
                        pm.remove(&mut txn, peer_str.as_str());
                    });
                }
            }
        });
        std::mem::forget(self_evict_sub);
    }

    // Non-admin: observe member list for our own peer ID appearing (runtime approval
    // delivered via P2P sync). When the admin approves us, they write our entry into
    // the member list; the observer fires and we remove our own pending entry so it
    // stops showing us as "pending" in the UI.
    {
        let is_local_admin = cdir.join("admin.key").exists();
        if !is_local_admin {
            let member_map = state.control.get_or_insert_map(MEMBER_LIST_KEY);
            let self_peer_str = peer_id.to_string();
            let state_for_approval = state.clone();
            let approval_sub = member_map.observe(move |txn: &yrs::TransactionMut, event: &yrs::types::map::MapEvent| {
                let is_p2p = txn.origin().map(|o| o.as_ref() == b"p2p").unwrap_or(false);
                if !is_p2p { return; }
                for (key, change) in event.keys(txn) {
                    if key.as_ref() != self_peer_str.as_str() { continue; }
                    if let yrs::types::EntryChange::Inserted(_) = change {
                        // Admin just wrote our member entry via P2P sync — remove our pending entry.
                        let s = state_for_approval.clone();
                        let peer_str = self_peer_str.clone();
                        tokio::spawn(async move {
                            use yrs::{Map, Transact as _};
                            let pm = s.control.get_or_insert_map(MLS_PENDING_KEY);
                            let mut txn = s.control.transact_mut();
                            pm.remove(&mut txn, peer_str.as_str());
                            tracing::info!("[member] removed pending entry after P2P approval");
                        });
                    }
                }
            });
            std::mem::forget(approval_sub);
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
        let has_group = mls.lock().await.group.is_some();
        if !has_group {
            let welcomes_map = state.control.get_or_insert_map(MLS_WELCOMES_KEY);
            let our_peer_id_str = peer_id.to_string();
            let mls_w = mls.clone();
            let state_w = state.clone();
            let welcome_sub = welcomes_map.observe(move |txn: &yrs::TransactionMut, event: &yrs::types::map::MapEvent| {
                use yrs::types::EntryChange;
                for (key, change) in event.keys(txn) {
                    if key.as_ref() != our_peer_id_str.as_str() { continue; }
                    if let EntryChange::Inserted(yrs::Out::Any(yrs::Any::String(s))) = change {
                        let welcome_hex = s.to_string();
                        let mls = mls_w.clone();
                        let state = state_w.clone();
                        tokio::spawn(async move {
                            consume_welcome(welcome_hex, mls, state).await;
                        });
                    }
                }
            });
            std::mem::forget(welcome_sub);
        }
    }

    // ── Commit watcher ────────────────────────────────────────────────────────
    // All peers: watch mls_commits for new entries and apply them to keep MLS
    // group state in sync with epoch advances (membership tracking; the
    // transport PSK is NOT rotated — see docs/plan/identity.md).
    //
    // Commits are fed through a serial channel to prevent concurrent MLS
    // operations from racing — multiple commits arriving in a single P2P sync
    // batch would otherwise spawn concurrent tasks fighting over the mutex.
    {
        use yrs::types::Change;
        let (commit_tx, mut commit_rx) =
            tokio::sync::mpsc::unbounded_channel::<MlsCommitEntry>();
        let commits_arr = state.control.get_or_insert_array(MLS_COMMITS_KEY);
        let commits_sub = commits_arr.observe(move |txn: &yrs::TransactionMut, event: &yrs::types::array::ArrayEvent| {
            let is_p2p = txn.origin().map(|o| o.as_ref() == b"p2p").unwrap_or(false);
            if !is_p2p { return; }
            for change in event.delta(txn) {
                #[allow(clippy::collapsible_match)]
                if let Change::Added(values) = change {
                    for val in values {
                        if let yrs::Out::Any(yrs::Any::String(s)) = val {
                            if let Ok(entry) = serde_json::from_str::<MlsCommitEntry>(s) {
                                let _ = commit_tx.send(entry);
                            }
                        }
                    }
                }
            }
        });
        std::mem::forget(commits_sub);

        let mls_c = mls.clone();
        let state_c = state.clone();
        let token_c = token.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = token_c.cancelled() => break,
                    entry = commit_rx.recv() => match entry {
                        Some(e) => apply_commit_entry(e, mls_c.clone(), state_c.clone()).await,
                        None => break,
                    },
                }
            }
        });
    }

    spawn_watcher(state.clone(), workspace, token.clone()).await?;
    crate::proposal::engine::spawn_engine(state.clone(), token.clone());
    crate::agent::reaction::spawn_reaction(state.clone(), token.clone());
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
                    "/enoxian/1.0.0".to_string(),
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

    // TCP (PSK-protected, for LAN and relay) on a STABLE per-circle port; QUIC
    // (UDP, for WAN/NAT hole-punching via DCUtR) on an ephemeral port.
    //
    // LAN peers discover each other via mDNS regardless of port, and behind NAT
    // we rely on the relay/rendezvous + the QUIC ExternalAddrConfirmed address.
    // But peers reached over a stable-IP overlay (e.g. Tailscale) WITHOUT a
    // rendezvous server only have the bootstrap address saved at `enox enter`
    // time — so that address must survive daemon restarts. A deterministic TCP
    // port keeps it valid; we fall back to ephemeral if the port is taken.
    //
    // QUIC stays ephemeral: a fixed UDP port buys nothing here (hole-punching
    // discovers addresses dynamically), and the combined relay+quic transport
    // rejects a fixed-port QUIC listen, so binding one would just fail anyway.
    let listen_port = stable_listen_port(&config.circle_id);
    let tcp_addr = format!("/ip4/0.0.0.0/tcp/{listen_port}").parse::<Multiaddr>()?;
    if let Err(e) = swarm.listen_on(tcp_addr) {
        warn!(
            "[{}] stable TCP listen port {listen_port} unavailable ({e}); falling back to ephemeral",
            config.circle_id
        );
        swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse::<Multiaddr>()?)?;
    }
    swarm.listen_on("/ip4/0.0.0.0/udp/0/quic-v1".parse::<Multiaddr>()?)?;

    // ── Dial bootstrap peers from config ──────────────────────────────────────
    // Peer addresses saved at `enox enter` time (from invite). This ensures
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
    //
    // Configured relays are reserved immediately. If none are configured we fall
    // back to the DEFAULT_RELAY server resolved in the background — this is the
    // WAN fallback between rendezvous (discovery) and LAN-only (mDNS).
    let (relay_tx, mut relay_rx) = tokio::sync::mpsc::channel::<Multiaddr>(4);

    let mut reserved_any_relay = false;
    for relay_str in &config.relay_addrs {
        match relay_str.parse::<Multiaddr>() {
            Ok(relay_addr) => {
                let circuit_addr = relay_addr
                    .clone()
                    .with(libp2p::multiaddr::Protocol::P2pCircuit);
                info!("[{}] reserving relay slot at {relay_addr}", config.circle_id);
                if let Err(e) = swarm.listen_on(circuit_addr) {
                    warn!("[{}] relay circuit listen failed: {e}", config.circle_id);
                } else {
                    reserved_any_relay = true;
                }
            }
            Err(e) => warn!("[{}] invalid relay addr '{}': {e}", config.circle_id, relay_str),
        }
    }

    // If no relay configured, resolve the default relay server in the background
    // (WAN fallback — peers behind NAT stay reachable even without rendezvous).
    if !reserved_any_relay && crate::defaults::DEFAULT_RELAY.is_some() {
        let tx = relay_tx.clone();
        let cid = config.circle_id.clone();
        tokio::spawn(async move {
            match crate::commands::rendezvous::resolve_default_relay().await {
                Some(addr_str) => match addr_str.parse::<Multiaddr>() {
                    Ok(addr) => {
                        info!("[{cid}] resolved default relay: {addr_str}");
                        let _ = tx.send(addr).await;
                    }
                    Err(e) => warn!("[{cid}] invalid resolved relay addr: {e}"),
                },
                None => warn!("[{cid}] default relay unreachable — no WAN relay fallback"),
            }
        });
    }

    // ── Dial rendezvous servers (QUIC) ────────────────────────────────────────
    // Rendezvous servers speak QUIC without PSK. After connecting we register
    // under the circle UUID namespace and discover other members.
    //
    // Configured rendezvous addrs are dialed immediately (synchronous path).
    // If none are configured, the default server (DEFAULT_RENDEZVOUS) is resolved
    // in a background task so spawn_circle returns without blocking on a network
    // call — this prevents api/enter from timing out when the server is unreachable.
    //
    // The channel rdvz_tx/rdvz_rx lets the background task inject the resolved
    // address into the running swarm event loop.
    let rendezvous_peers: std::sync::Arc<std::sync::RwLock<HashSet<PeerId>>> =
        std::sync::Arc::new(std::sync::RwLock::new(HashSet::new()));

    let (rdvz_tx, mut rdvz_rx) = tokio::sync::mpsc::channel::<(Multiaddr, PeerId)>(4);

    // Dial any explicitly-configured rendezvous servers right now.
    for rdvz_str in &config.rendezvous_addrs {
        match rdvz_str.parse::<Multiaddr>() {
            Ok(addr) => {
                if let Some(pid) = addr.iter().find_map(|p| {
                    if let libp2p::multiaddr::Protocol::P2p(id) = p { Some(id) } else { None }
                }) {
                    rendezvous_peers.write().unwrap().insert(pid);
                }
                info!("[{}] dialing rendezvous server {addr}", config.circle_id);
                let _ = swarm.dial(addr);
            }
            Err(e) => warn!("[{}] invalid rendezvous addr '{}': {e}", config.circle_id, rdvz_str),
        }
    }

    // If no rendezvous configured, resolve the default server in the background.
    if config.rendezvous_addrs.is_empty() && crate::defaults::DEFAULT_RENDEZVOUS.is_some() {
        let tx = rdvz_tx.clone();
        let cid = config.circle_id.clone();
        tokio::spawn(async move {
            match crate::commands::rendezvous::resolve_default().await {
                Some(addr_str) => match addr_str.parse::<Multiaddr>() {
                    Ok(addr) => {
                        let peer_id = addr.iter().find_map(|p| {
                            if let libp2p::multiaddr::Protocol::P2p(id) = p { Some(id) } else { None }
                        });
                        if let Some(pid) = peer_id {
                            info!("[{cid}] resolved default rendezvous: {addr_str}");
                            let _ = tx.send((addr, pid)).await;
                        } else {
                            warn!("[{cid}] default rendezvous addr has no peer ID: {addr_str}");
                        }
                    }
                    Err(e) => warn!("[{cid}] invalid resolved rendezvous addr: {e}"),
                },
                None => warn!("[{cid}] default rendezvous unreachable — LAN-only mode"),
            }
        });
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

    // ── Accept incoming proposal-sync streams ─────────────────────────────────
    let mut proposal_accept_ctrl = swarm.behaviour().stream.new_control();
    let state_for_proposals = state.clone();
    let proposal_accept_token = token.clone();
    tokio::spawn(async move {
        let mut incoming = match proposal_accept_ctrl.accept(proposal_sync::PROTOCOL) {
            Ok(s) => s,
            Err(e) => { warn!("[proposal-sync] accept failed: {e}"); return; }
        };
        loop {
            tokio::select! {
                _ = proposal_accept_token.cancelled() => break,
                item = incoming.next() => match item {
                    Some((peer_id, stream)) => {
                        let s = state_for_proposals.clone();
                        tokio::spawn(proposal_sync::run(peer_id, stream, s, false));
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
        .unwrap_or_else(|_| rendezvous::Namespace::from_static("enoxian"));

    tokio::spawn(async move {
        // Re-register with rendezvous servers every hour (TTL is 2h).
        let mut reregister = tokio::time::interval(std::time::Duration::from_secs(3600));
        reregister.tick().await; // skip the immediate first tick

        loop {
            tokio::select! {
                _ = swarm_token.cancelled() => {
                    info!("[{}] circle stopped", circle_id);
                    // Write OFFLINE before dropping the swarm so connected peers
                    // receive the CRDT update over the still-open connections.
                    presence::write_offline(&state_for_swarm, &state_for_swarm.agent_id);
                    break;
                }
                // Background-resolved rendezvous address arrived (e.g. default server).
                item = rdvz_rx.recv() => {
                    if let Some((addr, peer_id)) = item {
                        rendezvous_peers.write().unwrap().insert(peer_id);
                        info!("[{}] dialing background-resolved rendezvous: {addr}", circle_id);
                        let _ = swarm.dial(addr);
                    }
                }
                // Background-resolved relay address arrived — reserve circuit slot (WAN fallback).
                item = relay_rx.recv() => {
                    if let Some(relay_addr) = item {
                        let circuit_addr = relay_addr
                            .clone()
                            .with(libp2p::multiaddr::Protocol::P2pCircuit);
                        info!("[{}] reserving background-resolved relay slot: {relay_addr}", circle_id);
                        if let Err(e) = swarm.listen_on(circuit_addr) {
                            warn!("[{}] relay circuit listen failed: {e}", circle_id);
                        }
                    }
                }
                _ = reregister.tick() => {
                    for &rdvz_peer in &*rendezvous_peers.read().unwrap() {
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
                        if rendezvous_peers.read().unwrap().contains(&peer_id) {
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
                            if !rendezvous_peers.read().unwrap().contains(&peer_id) {
                                let mut ctrl = open_ctrl.clone();
                                let s = state_for_swarm.clone();
                                tokio::spawn(async move {
                                    match ctrl.open_stream(peer_id, sync::PROTOCOL).await {
                                        Ok(stream) => sync::run_sync(peer_id, stream, s, true).await,
                                        Err(e) => warn!("[sync] open_stream to {peer_id}: {e}"),
                                    }
                                });
                                // Reconcile proposal history once per connection.
                                let mut pctrl = open_ctrl.clone();
                                let ps = state_for_swarm.clone();
                                tokio::spawn(async move {
                                    match pctrl.open_stream(peer_id, proposal_sync::PROTOCOL).await {
                                        Ok(stream) => proposal_sync::run(peer_id, stream, ps, true).await,
                                        Err(e) => warn!("[proposal-sync] open_stream to {peer_id}: {e}"),
                                    }
                                });
                            }
                        }
                    }
                    SwarmEvent::ConnectionClosed { peer_id, cause, num_established, .. } => {
                        info!("[{}] P2P disconnected: {peer_id}: {cause:?}", circle_id);
                        // When the last connection to this peer closes, immediately mark
                        // them offline in the shared presence CRDT so all peers see it
                        // right away — no need to wait for the heartbeat to time out.
                        if num_established == 0 {
                            use yrs::{Map, Out, Any, Transact};
                            let member_map = state_for_swarm.control.get_or_insert_map(MEMBER_LIST_KEY);
                            let txn = state_for_swarm.control.transact();
                            let agent_id_for_peer = member_map
                                .get(&txn, peer_id.to_string().as_str())
                                .and_then(|v| if let Out::Any(Any::String(s)) = v {
                                    serde_json::from_str::<MemberEntry>(&s).ok().map(|m| m.agent_id)
                                } else { None });
                            drop(txn);
                            if let Some(agent_id) = agent_id_for_peer {
                                presence::write_offline(&state_for_swarm, &agent_id);
                            }
                        }
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
                        let msg = error.to_string();
                        // A connection reset during/just after the noise handshake (right
                        // after the pnet pre-shared-key cipher is set up) almost always
                        // means the two sides have different circle PSKs — the dialed
                        // address belongs to a different circle, or one side re-created
                        // the circle with a new key. pnet is unauthenticated, so the
                        // cipher "succeeds" and the failure only shows up here.
                        let hint = if msg.contains("reset") || msg.contains("ConnectionReset") {
                            " — likely PSK mismatch (different circle key, or the dialed address belongs to another circle)"
                        } else {
                            ""
                        };
                        tracing::warn!("[{}] outgoing connection to {peer_id:?} failed: {error}{hint}", circle_id);
                        state_for_swarm.record_conn_error(format!("{error}{hint}"));
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
        Err(_) => {
            // add_member failed — most likely the peer is already in the MLS group
            // (e.g. the daemon restarted and re-wrote a pending entry to the CRDT
            // before the sync caught up). Remove the stale pending entry so the UI
            // stops showing them as awaiting approval.
            let member_map = state.control.get_or_insert_map(MEMBER_LIST_KEY);
            let pending_map = state.control.get_or_insert_map(MLS_PENDING_KEY);
            let already_member = {
                use yrs::Transact;
                let txn = state.control.transact();
                matches!(
                    member_map.get(&txn, peer_id_str.as_str()),
                    Some(yrs::Out::Any(yrs::Any::String(_)))
                )
            };
            if already_member {
                use yrs::Transact;
                let mut txn = state.control.transact_mut();
                pending_map.remove(&mut txn, peer_id_str.as_str());
                info!("[member] removed stale pending entry for {peer_id_str} (already a member)");
            }
            return;
        }
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
        let (owner, agent_id, device_label, agents) = pending_entry
            .map(|p| (p.owner, p.agent_id, p.device_label, p.agents))
            .unwrap_or_default();
        let msg = format!("add:{}:member:owner:{}", peer_id_str, owner);
        let entry = MemberEntry {
            peer_id: peer_id_str.clone(),
            owner,
            agent_id,
            device_label,
            agents,
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
) {
    let welcome_bytes = match hex::decode(&welcome_hex) {
        Ok(b) => b,
        Err(_) => return,
    };

    // Join the MLS group and persist it. We deliberately do NOT derive an epoch
    // PSK or rotate the transport key here: the transport PSK is a stable
    // per-circle network gate, and eviction is enforced by the mls_removed
    // sync-gate (see docs/plan/identity.md). MLS membership is still tracked for
    // the sync gate and for future content-layer encryption.
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
    let _ = group.save(identity, &state.circle_dir);
    mls_locked.group = Some(group);
    info!("[mls] joined group via Welcome (membership tracked; transport PSK stays stable)");
}

/// Called for every new MlsCommitEntry that arrives from a peer.
/// Skips commits already applied (epoch < current), skips our own commits,
/// and does nothing if the group becomes inactive (we were removed — we'll
/// be locked out naturally when others rotate to the new PSK).
async fn apply_commit_entry(
    entry: MlsCommitEntry,
    mls: SharedMlsState,
    state: AppState,
) {
    // Don't apply commits we ourselves produced.
    if entry.sender_peer_id == state.peer_id { return; }

    let commit_bytes = match hex::decode(&entry.data_hex) {
        Ok(b) => b,
        Err(_) => return,
    };

    // Apply the commit to keep our MLS group state in sync (epoch advances are
    // tracked for the sync gate and future content encryption). We do NOT derive
    // an epoch PSK or rotate the transport key — the transport PSK is a stable
    // per-circle gate and eviction is the mls_removed sync-gate. See
    // docs/plan/identity.md.
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
        Ok(()) => {
            let _ = group.save(identity, &state.circle_dir);
            info!("[mls] applied Commit epoch {} → {} (membership tracked)", entry.epoch, group.epoch());
        }
        Err(e) => { warn!("[mls] apply_commit (epoch {}): {e}", entry.epoch); }
    }
}

/// Saves the new PSK to config.toml, then stops and restarts the circle so
/// the swarm rebuilds its transport with the new pnet key.
///
/// NOTE: no longer wired to MLS epoch changes — the transport PSK is a stable
/// per-circle gate and eviction is the mls_removed sync-gate (see
/// docs/plan/identity.md). Retained for a possible future *explicit* circle-key
/// rotation (e.g. a manual `enox rotate-key` admin action); currently unused.
#[allow(dead_code)]
pub async fn rotate_psk_and_restart(circle_id: &str, new_psk: [u8; 32], daemon: DaemonState) {
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

/// Deterministic TCP listen port for a circle, in the IANA dynamic/private
/// range (49152–61151). Derived from the circle_id via FNV-1a so every device
/// in the circle is predictable and the same across daemon restarts — which is
/// what keeps a saved Tailscale/LAN bootstrap address valid without a
/// rendezvous server. Collisions with other processes fall back to ephemeral
/// at bind time (see spawn_circle).
fn stable_listen_port(circle_id: &str) -> u16 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in circle_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    49_152 + (hash % 12_000) as u16
}

/// Returns true for listen addresses worth tracking for invite embedding:
/// rejects loopback, unspecified, link-local, and p2p-circuit relay addresses.
/// RFC1918 and Tailscale CGNAT addresses are kept — `enox invite` sorts them
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
            Protocol::Ip6(ip) if ip.is_loopback() || ip.is_unspecified() => {
                return false;
            }
            _ => {}
        }
    }
    true
}
