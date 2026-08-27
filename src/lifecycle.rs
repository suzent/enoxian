//! Per-circle spawn logic — called at daemon startup, on hot-reload, and from the
//! `POST /circles/<id>/start` API endpoint.

use anyhow::Result;
use libp2p::{
    core::muxing::StreamMuxerBox,
    dcutr,
    futures::StreamExt,
    identify, kad, mdns, noise, pnet, quic, relay, rendezvous,
    swarm::{
        behaviour::toggle::Toggle,
        dial_opts::{DialOpts, PeerCondition},
        SwarmEvent,
    },
    tcp, yamux, Multiaddr, PeerId, SwarmBuilder,
};
use std::collections::{HashMap, HashSet};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Retry budget for clearing a peer's pending entry. See [`remove_pending_entry`].
const PENDING_REMOVE_RETRIES: u32 = 10;
const PENDING_REMOVE_BACKOFF: std::time::Duration = std::time::Duration::from_millis(50);

use yrs::{Array, Observable};

use crate::{
    config::{self, CircleConfig, JoinPolicy},
    control::{
        MemberEntry, MemberRole, MlsCommitEntry, OwnerClaim, PendingEntry, MEMBER_LIST_KEY,
        MLS_COMMITS_KEY, MLS_KEY_PACKAGES_KEY, MLS_OWNER_CLAIMS_KEY, MLS_PENDING_KEY,
        MLS_WELCOMES_KEY,
    },
    crypto::{keypair_from_hex, psk_from_hex},
    daemon::DaemonState,
    mls::{MlsGroupManager, MlsIdentity, SharedMlsState},
    network::{
        behaviour::{EnochBehaviour, EnochEvent},
        event_sync, mls_bootstrap, proposal_sync, sync,
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
    let force_relay = config.force_relay;
    let keypair = keypair_from_hex(&config.keypair_proto_hex)?;
    let peer_id = keypair.public().to_peer_id();
    let psk_bytes = psk_from_hex(&config.psk_hex)?;

    let workspace = if config.workspace_dir.is_empty() {
        crate::config::circle_dir(&config.circle_id)?.join("files")
    } else {
        std::path::PathBuf::from(&config.workspace_dir)
    };
    let workspace = crate::config::normalize_workspace_dir(&workspace)?;
    for active in daemon.list() {
        if active.circle_id != config.circle_id
            && crate::config::workspace_paths_equal(&active.workspace, &workspace)?
        {
            anyhow::bail!(
                "workspace {} is already active in circle '{}' ({})",
                workspace.display(),
                active.circle_name,
                active.circle_id
            );
        }
    }
    tokio::fs::create_dir_all(&workspace).await?;

    info!(
        "  Circle '{}' ({}) — PeerID: {} — Workspace: {}",
        config.circle_name,
        config.circle_id,
        peer_id,
        workspace.display()
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

    // Restore persisted coordination state (chat / tasks / members) BEFORE the
    // swarm connects and before observers/reaction loop start, so a cold-started
    // circle keeps its history even if no peer is online to re-sync. Restored
    // chat carries its original (old) timestamps, so the agent reaction loop's
    // `ts` cutoff skips it — a restored mention never re-triggers an agent.
    if let Err(e) = crate::store::control::restore(&cdir, &state.control) {
        warn!("[control] restore failed: {e}");
    }

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
        let owner_sig = keypair
            .sign(owner_claim_msg.as_bytes())
            .map(hex::encode)
            .unwrap_or_default();
        let claim = OwnerClaim {
            owner: config.owner.clone(),
            sig: owner_sig,
        };
        if let Ok(json_str) = serde_json::to_string(&claim) {
            let claims_map = state.control.get_or_insert_map(MLS_OWNER_CLAIMS_KEY);
            let mut txn = state.control.transact_mut();
            claims_map.insert(&mut txn, peer_id.to_string().as_str(), json_str.as_str());
        }
    }

    // Auto-register local peer in the member list so `enox member list` shows all participants.
    // Only writes if no entry exists yet — preserves explicit removals across restarts.
    {
        use yrs::{Any, Map, Out, Transact};
        let map = state.control.get_or_insert_map(MEMBER_LIST_KEY);
        let already_registered = {
            let txn = state.control.transact();
            matches!(
                map.get(&txn, peer_id.to_string().as_str()),
                Some(Out::Any(Any::String(_)))
            )
        };
        if !already_registered {
            let is_local_admin = cdir.join("admin.key").exists();
            let role = if is_local_admin {
                MemberRole::Admin
            } else {
                MemberRole::Member
            };
            let msg = format!("add:{peer_id}:{role}");
            let signature = keypair
                .sign(msg.as_bytes())
                .map(hex::encode)
                .unwrap_or_default();
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
                            if key == peer_id.to_string().as_str() {
                                return None;
                            }
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
                        keypair
                            .sign(owner_claim_msg.as_bytes())
                            .map(hex::encode)
                            .unwrap_or_default()
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

            // Refresh our advertised agents / device label if they've changed
            // since we joined (e.g. agents added to agents.toml after the first
            // join). Without this, a device that configured agents later would
            // keep advertising an empty list, so mentions couldn't target it.
            let self_key = peer_id.to_string();
            let current_agents = crate::identity::read_local_agents();
            let current_label = crate::identity::read_identity_display()
                .map(|(label, _)| label)
                .unwrap_or_default();
            let existing: Option<MemberEntry> = {
                let txn = state.control.transact();
                match map.get(&txn, self_key.as_str()) {
                    Some(Out::Any(Any::String(s))) => serde_json::from_str(&s).ok(),
                    _ => None,
                }
            };
            if let Some(mut entry) = existing {
                if entry.agents != current_agents || entry.device_label != current_label {
                    entry.agents = current_agents;
                    entry.device_label = current_label;
                    if let Ok(json_str) = serde_json::to_string(&entry) {
                        let mut txn = state.control.transact_mut();
                        map.insert(&mut txn, self_key.as_str(), json_str.as_str());
                        info!("[member] refreshed advertised agents/label for self");
                    }
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
            if matches!(
                pending_map.get(&txn, self_peer_str.as_str()),
                Some(Out::Any(_))
            ) {
                drop(txn);
                let mut txn = state.control.transact_mut();
                pending_map.remove(&mut txn, self_peer_str.as_str());
            }
        }

        // Case 2: arrives later via P2P sync. Observe and evict immediately.
        let state_for_self_evict = state.clone();
        let self_evict_sub = pending_map.observe(
            move |txn: &yrs::TransactionMut, event: &yrs::types::map::MapEvent| {
                let is_p2p = txn.origin().map(|o| o.as_ref() == b"p2p").unwrap_or(false);
                if !is_p2p {
                    return;
                }
                for (key, change) in event.keys(txn) {
                    if key.as_ref() != self_peer_str.as_str() {
                        continue;
                    }
                    if let yrs::types::EntryChange::Inserted(_) = change {
                        // Our own peer ID was just inserted by a remote — remove it.
                        let s = state_for_self_evict.clone();
                        let peer_str = self_peer_str.clone();
                        tokio::spawn(async move {
                            remove_pending_entry(&s, peer_str.as_str(), "self-evict").await;
                        });
                    }
                }
            },
        );
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
            let approval_sub = member_map.observe(
                move |txn: &yrs::TransactionMut, event: &yrs::types::map::MapEvent| {
                    let is_p2p = txn.origin().map(|o| o.as_ref() == b"p2p").unwrap_or(false);
                    if !is_p2p {
                        return;
                    }
                    for (key, change) in event.keys(txn) {
                        if key.as_ref() != self_peer_str.as_str() {
                            continue;
                        }
                        if approval_clears_pending(change) {
                            // The admin wrote our member entry via P2P sync — drop
                            // our own pending entry.
                            let s = state_for_approval.clone();
                            let peer_str = self_peer_str.clone();
                            tokio::spawn(async move {
                                remove_pending_entry(&s, peer_str.as_str(), "approved via P2P")
                                    .await;
                            });
                        }
                    }
                },
            );
            std::mem::forget(approval_sub);
        }
    }

    // If admin and auto join policy, observe pending map
    let is_admin = cdir.join("admin.key").exists();
    if is_admin && config.join_policy == JoinPolicy::Auto {
        let pending_map = state.control.get_or_insert_map(MLS_PENDING_KEY);
        let state_for_pending = state.clone();
        let mls_for_pending = mls.clone();
        let pending_sub = pending_map.observe(
            move |txn: &yrs::TransactionMut, event: &yrs::types::map::MapEvent| {
                let is_p2p = txn.origin().map(|o| o.as_ref() == b"p2p").unwrap_or(false);
                if !is_p2p {
                    return;
                }
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
            },
        );
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
            let welcome_sub = welcomes_map.observe(
                move |txn: &yrs::TransactionMut, event: &yrs::types::map::MapEvent| {
                    use yrs::types::EntryChange;
                    for (key, change) in event.keys(txn) {
                        if key.as_ref() != our_peer_id_str.as_str() {
                            continue;
                        }
                        if let EntryChange::Inserted(yrs::Out::Any(yrs::Any::String(s))) = change {
                            let welcome_hex = s.to_string();
                            let mls = mls_w.clone();
                            let state = state_w.clone();
                            tokio::spawn(async move {
                                consume_welcome(welcome_hex, mls, state).await;
                            });
                        }
                    }
                },
            );
            std::mem::forget(welcome_sub);
        }
    }

    // ── Commit watcher ────────────────────────────────────────────────────────
    // All peers: watch mls_commits for new entries and apply them to keep MLS
    // group state in sync with epoch advances (membership tracking; the
    // transport PSK is NOT rotated — see docs/concepts/security.md).
    //
    // Commits are fed through a serial channel to prevent concurrent MLS
    // operations from racing — multiple commits arriving in a single P2P sync
    // batch would otherwise spawn concurrent tasks fighting over the mutex.
    {
        use yrs::types::Change;
        let (commit_tx, mut commit_rx) = tokio::sync::mpsc::unbounded_channel::<MlsCommitEntry>();
        let commits_arr = state.control.get_or_insert_array(MLS_COMMITS_KEY);
        let commits_sub = commits_arr.observe(
            move |txn: &yrs::TransactionMut, event: &yrs::types::array::ArrayEvent| {
                let is_p2p = txn.origin().map(|o| o.as_ref() == b"p2p").unwrap_or(false);
                if !is_p2p {
                    return;
                }
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
            },
        );
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
    // Upgrade pre-M15 proposal history into the append-only event log before
    // any peer event stream starts. Fresh proposals append events themselves.
    match (
        crate::proposal::store::ProposalStore::open(&state.workspace),
        crate::workspace_event::EventStore::open(&state.workspace, state.circle_id.clone()),
    ) {
        (Ok(proposals), Ok(events)) => {
            let device = crate::identity::read_identity_display()
                .and_then(|(_, device_label)| device_label)
                .unwrap_or_else(|| state.agent_id.clone());
            if let Err(error) = events.backfill_proposals(&proposals, &state.peer_id, &device) {
                warn!("[workspace-event] proposal backfill failed: {error}");
            }
        }
        (Err(error), _) | (_, Err(error)) => {
            warn!("[workspace-event] store initialization failed: {error}");
        }
    }
    crate::proposal::engine::spawn_engine(state.clone(), token.clone());
    crate::agent::reaction::spawn_reaction(state.clone(), token.clone());
    presence::spawn_presence(state.clone(), agent_id, token.clone());
    spawn_control_persist(state.clone(), cdir.clone(), token.clone());

    // ── Build the P2P swarm ───────────────────────────────────────────────────
    let pnet_config = pnet::PnetConfig::new(pnet::PreSharedKey::new(psk_bytes));
    let keypair_clone = keypair.clone();
    let relay_peer_ids = crate::network::public_relay_transport::relay_peer_ids_from_addrs(
        config.relay_addrs.iter(),
    );
    let relay_base_addrs = std::sync::Arc::new(std::sync::RwLock::new(
        config
            .relay_addrs
            .iter()
            .filter_map(|addr| addr.parse::<Multiaddr>().ok())
            .filter_map(|addr| {
                crate::network::public_relay_transport::relay_peer_id(&addr)
                    .map(|peer_id| (peer_id, addr))
            })
            .collect::<HashMap<_, _>>(),
    ));
    let mut public_relay_peer_ids = relay_peer_ids.clone();
    public_relay_peer_ids.extend(
        crate::network::public_relay_transport::relay_peer_ids_from_addrs(
            config.rendezvous_addrs.iter(),
        ),
    );
    let public_relay_peer_ids = std::sync::Arc::new(std::sync::RwLock::new(public_relay_peer_ids));
    let public_relay_peer_ids_for_transport = public_relay_peer_ids.clone();

    // relay::client::new produces the relay transport (for dialing circuits) and
    // the relay client behaviour (for managing reservations).
    let (relay_transport, relay_client_behaviour) = relay::client::new(peer_id);

    let mut swarm = SwarmBuilder::with_existing_identity(keypair.clone())
        .with_tokio()
        .with_other_transport(move |key| {
            use futures::future::Either;
            use libp2p::{core::upgrade, Transport};

            // Public TCP without pnet: only allowed for known relay server peer IDs.
            // This lets us reserve circuit slots on public infrastructure that does
            // not know the circle PSK, while keeping direct peer TCP PSK-protected.
            let public_tcp = crate::network::public_relay_transport::PublicRelayTransport::new(
                tcp::tokio::Transport::new(tcp::Config::default()),
                public_relay_peer_ids_for_transport.clone(),
            )
            .upgrade(upgrade::Version::V1Lazy)
            .authenticate(noise::Config::new(key)?)
            .multiplex(yamux::Config::default())
            .map(|(id, muxer), _| (id, StreamMuxerBox::new(muxer)));

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
        .with_behaviour(move |key| {
            let pid = key.public().to_peer_id();
            Ok(EnochBehaviour {
                // Disabled: see EnochBehaviour::mdns. Re-enable by wrapping
                // `mdns::tokio::Behaviour::new(mdns::Config::default(), pid)?` in
                // `Toggle::from(Some(..))`.
                mdns: Toggle::from(None::<mdns::tokio::Behaviour>),
                kad: {
                    let mut kad = kad::Behaviour::new(pid, kad::store::MemoryStore::new(pid));
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
                dcutr: Toggle::from((!force_relay).then(|| dcutr::Behaviour::new(pid))),
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
    if force_relay {
        info!(
            "[{}] force-relay mode: direct TCP/QUIC listeners and DCUtR are disabled",
            config.circle_id
        );
    } else {
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
    }

    // ── Dial bootstrap peers from config ──────────────────────────────────────
    // Peer addresses saved at `enox enter` time (from invite). This ensures
    // connectivity even when mDNS is unavailable (different subnets, firewalls).
    for peer_str in config.peers.iter().filter(|_| !force_relay) {
        match peer_str.parse::<Multiaddr>() {
            Ok(addr) => {
                info!("[{}] dialing bootstrap peer {addr}", config.circle_id);
                let _ = swarm.dial(addr);
            }
            Err(e) => warn!(
                "[{}] invalid peer addr '{}': {e}",
                config.circle_id, peer_str
            ),
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
                info!(
                    "[{}] reserving relay slot at {relay_addr}",
                    config.circle_id
                );
                if let Err(e) = swarm.listen_on(circuit_addr) {
                    warn!("[{}] relay circuit listen failed: {e}", config.circle_id);
                } else {
                    reserved_any_relay = true;
                }
            }
            Err(e) => warn!(
                "[{}] invalid relay addr '{}': {e}",
                config.circle_id, relay_str
            ),
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
                    if let libp2p::multiaddr::Protocol::P2p(id) = p {
                        Some(id)
                    } else {
                        None
                    }
                }) {
                    rendezvous_peers.write().unwrap().insert(pid);
                    if relay_peer_ids.contains(&pid) {
                        info!(
                            "[{}] rendezvous shares relay peer {pid}; using relay TCP connection",
                            config.circle_id
                        );
                        continue;
                    }
                }
                info!("[{}] dialing rendezvous server {addr}", config.circle_id);
                let _ = swarm.dial(addr);
            }
            Err(e) => warn!(
                "[{}] invalid rendezvous addr '{}': {e}",
                config.circle_id, rdvz_str
            ),
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
                            if let libp2p::multiaddr::Protocol::P2p(id) = p {
                                Some(id)
                            } else {
                                None
                            }
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

    // ── Accept the narrow MLS membership bootstrap stream ────────────────────
    let mut bootstrap_control = swarm.behaviour().stream.new_control();
    let state_for_bootstrap = state.clone();
    let bootstrap_token = token.clone();
    tokio::spawn(async move {
        let mut incoming = match bootstrap_control.accept(mls_bootstrap::PROTOCOL) {
            Ok(streams) => streams,
            Err(error) => {
                warn!("[mls-bootstrap] accept failed: {error}");
                return;
            }
        };
        loop {
            tokio::select! {
                _ = bootstrap_token.cancelled() => break,
                item = incoming.next() => match item {
                    Some((peer_id, stream)) => {
                        let state = state_for_bootstrap.clone();
                        tokio::spawn(mls_bootstrap::run(peer_id, stream, state, false));
                    }
                    None => break,
                }
            }
        }
    });

    // ── Accept incoming encrypted sync streams ────────────────────────────────
    let mut stream_control = swarm.behaviour().stream.new_control();
    let state_for_accept = state.clone();
    let accept_token = token.clone();
    tokio::spawn(async move {
        let mut incoming = match stream_control.accept(sync::PROTOCOL) {
            Ok(s) => s,
            Err(e) => {
                warn!("[stream] accept failed: {e}");
                return;
            }
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
            Err(e) => {
                warn!("[proposal-sync] accept failed: {e}");
                return;
            }
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

    // ── Accept persistent workspace-event streams ────────────────────────────
    let mut event_accept_ctrl = swarm.behaviour().stream.new_control();
    let state_for_events = state.clone();
    let event_accept_token = token.clone();
    tokio::spawn(async move {
        let mut incoming = match event_accept_ctrl.accept(event_sync::PROTOCOL) {
            Ok(streams) => streams,
            Err(error) => {
                warn!("[event-sync] accept failed: {error}");
                return;
            }
        };
        loop {
            tokio::select! {
                _ = event_accept_token.cancelled() => break,
                item = incoming.next() => match item {
                    Some((peer_id, stream)) => {
                        let state = state_for_events.clone();
                        tokio::spawn(event_sync::run(peer_id, stream, state, false));
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

        // The background resolver tasks feeding these channels drop their senders
        // once they finish (resolve or fail). A closed `mpsc::Receiver` returns
        // `recv() => Ready(None)` *immediately and forever*, so without these
        // guards the `select!` would spin at 100% CPU polling a dead channel.
        // Disable each branch the first time its channel closes.
        let mut rdvz_open = true;
        let mut relay_open = true;

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
                item = rdvz_rx.recv(), if rdvz_open => match item {
                    Some((addr, peer_id)) => {
                        rendezvous_peers.write().unwrap().insert(peer_id);
                        info!("[{}] dialing background-resolved rendezvous: {addr}", circle_id);
                        let _ = swarm.dial(addr);
                    }
                    None => rdvz_open = false, // sender dropped — stop polling this branch
                },
                // Background-resolved relay address arrived — reserve circuit slot (WAN fallback).
                item = relay_rx.recv(), if relay_open => match item {
                    Some(relay_addr) => {
                        if let Some(peer_id) =
                            crate::network::public_relay_transport::relay_peer_id(&relay_addr)
                        {
                            public_relay_peer_ids.write().unwrap().insert(peer_id);
                            relay_base_addrs
                                .write()
                                .unwrap()
                                .insert(peer_id, relay_addr.clone());
                        }
                        let circuit_addr = relay_addr
                            .clone()
                            .with(libp2p::multiaddr::Protocol::P2pCircuit);
                        info!("[{}] reserving background-resolved relay slot: {relay_addr}", circle_id);
                        if let Err(e) = swarm.listen_on(circuit_addr) {
                            warn!("[{}] relay circuit listen failed: {e}", circle_id);
                        }
                    }
                    None => relay_open = false, // sender dropped — stop polling this branch
                },
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
                    SwarmEvent::ConnectionEstablished { peer_id, connection_id, endpoint, .. } => {
                        let remote_addr = endpoint.get_remote_address();
                        // For inbound relay circuits libp2p reports the remote address as
                        // only `/p2p/<source>`; the circuit marker lives on the local side.
                        let is_relayed = endpoint.is_relayed();
                        let is_infrastructure = rendezvous_peers.read().unwrap().contains(&peer_id)
                            || relay_base_addrs.read().unwrap().contains_key(&peer_id);
                        if force_relay
                            && !is_infrastructure
                            && !is_relayed
                        {
                            info!(
                                "[{}] force-relay mode: rejecting direct peer connection from {peer_id} via {remote_addr}",
                                circle_id
                            );
                            swarm.close_connection(connection_id);
                            continue;
                        }
                        // A peer that already identified itself as belonging to
                        // another circle is dropped before we spend any streams
                        // on it. Only confirmed mismatches are filtered here —
                        // an unknown peer may be a fresh joiner and must still
                        // be allowed to reach the sync handshake.
                        if !is_infrastructure
                            && state_for_swarm.is_foreign_peer(&peer_id.to_string())
                        {
                            tracing::debug!(
                                "[{}] dropping connection from {peer_id}: belongs to another circle",
                                circle_id
                            );
                            swarm.close_connection(connection_id);
                            continue;
                        }
                        let route_addr = match &endpoint {
                            libp2p::core::ConnectedPoint::Listener { local_addr, .. }
                                if is_relayed => local_addr,
                            _ => remote_addr,
                        };
                        info!("[{}] P2P connected: {peer_id} via {route_addr}", circle_id);
                        state_for_swarm.record_peer_connection(
                            peer_id.to_string(),
                            connection_id,
                            route_addr,
                        );
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
                                let mut bootstrap_ctrl = open_ctrl.clone();
                                let bootstrap_state = state_for_swarm.clone();
                                tokio::spawn(async move {
                                    match bootstrap_ctrl.open_stream(peer_id, mls_bootstrap::PROTOCOL).await {
                                        Ok(stream) => mls_bootstrap::run(peer_id, stream, bootstrap_state, true).await,
                                        Err(error) => warn!("[mls-bootstrap] open_stream to {peer_id}: {error}"),
                                    }
                                });
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
                                // Reconcile once, then keep the append-only M15
                                // event stream open for live decisions/conflicts.
                                let mut ectrl = open_ctrl.clone();
                                let es = state_for_swarm.clone();
                                tokio::spawn(async move {
                                    match ectrl.open_stream(peer_id, event_sync::PROTOCOL).await {
                                        Ok(stream) => event_sync::run(peer_id, stream, es, true).await,
                                        Err(e) => warn!("[event-sync] open_stream to {peer_id}: {e}"),
                                    }
                                });
                            }
                        }
                    }
                    SwarmEvent::ConnectionClosed { peer_id, connection_id, cause, num_established, .. } => {
                        info!("[{}] P2P disconnected: {peer_id}: {cause:?}", circle_id);
                        state_for_swarm.remove_peer_connection(
                            peer_id.to_string().as_str(),
                            connection_id,
                        );
                        // When the last connection to this peer closes, immediately mark
                        // them offline in the shared presence CRDT so all peers see it
                        // right away — no need to wait for the heartbeat to time out.
                        if num_established == 0 {
                            use yrs::{Map, Out, Any, ReadTxn, Transact};
                            let Ok(txn) = state_for_swarm.control.try_transact() else { continue };
                            let agent_id_for_peer = txn
                                .get_map(MEMBER_LIST_KEY)
                                .and_then(|member_map| member_map.get(&txn, peer_id.to_string().as_str()))
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
                            if state_for_swarm.is_foreign_peer(&peer_id.to_string()) {
                                continue;
                            }
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
                        // Don't feed another circle's peer into our routing
                        // table — that is what makes the redial loop survive
                        // long after the connection was rejected.
                        if state_for_swarm.is_foreign_peer(&peer_id.to_string()) {
                            continue;
                        }
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
                                    if state_for_swarm.is_foreign_peer(&pid.to_string()) {
                                        tracing::debug!(
                                            "[{}] skipping {pid}: belongs to another circle",
                                            circle_id
                                        );
                                        continue;
                                    }
                                    for addr in reg.record.addresses() {
                                        if force_relay
                                            && !crate::network::public_relay_transport::is_relayed_addr(addr)
                                        {
                                            tracing::debug!(
                                                "[{}] force-relay mode: ignoring direct rendezvous address {addr}",
                                                circle_id
                                            );
                                            continue;
                                        }
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
                        use relay::client::Event as RCE;
                        if let RCE::ReservationReqAccepted { relay_peer_id, .. } = e {
                            info!("[{}] relay reservation accepted at {relay_peer_id}", circle_id);
                            // Synthesize our circuit address and add it to our tracked external addresses.
                            // This ensures that when we re-register with rendezvous, we tell other peers
                            // "You can reach me by tunneling through this relay".
                            let relay_addr = relay_base_addrs
                                .read()
                                .unwrap()
                                .get(&relay_peer_id)
                                .cloned();

                            if let Some(mut base_addr) = relay_addr {
                                base_addr.push(libp2p::multiaddr::Protocol::P2pCircuit);
                                if let Ok(mut ext) = state_for_swarm.p2p_external_addrs.write() {
                                    let s = base_addr.to_string();
                                    if !ext.contains(&s) {
                                        ext.push(s);
                                        info!("[{}] added circuit to external addrs: {base_addr}", circle_id);
                                    }
                                }
                                // Immediately tell the swarm this is a valid external address for us
                                swarm.add_external_address(base_addr);

                                // Re-trigger rendezvous registration now that we have an external address
                                for &rdvz_peer in &*rendezvous_peers.read().unwrap() {
                                    if swarm.is_connected(&rdvz_peer) {
                                        let _ = swarm.behaviour_mut().rendezvous.register(
                                            rendezvous_namespace.clone(), rdvz_peer, None,
                                        );
                                    }
                                }
                            }
                        }
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

/// Periodically persist the durable control-doc state (chat/tasks/members) to
/// disk, and once more on clean shutdown. Debounced by a fixed interval — the
/// control doc changes often (presence heartbeats), but those are excluded from
/// the snapshot, so a periodic full save is cheap and simple. See
/// `crate::store::control`.
fn spawn_control_persist(
    state: AppState,
    circle_dir: std::path::PathBuf,
    token: CancellationToken,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        interval.tick().await; // skip the immediate first tick
        loop {
            tokio::select! {
                _ = token.cancelled() => {
                    // Final save on shutdown so the latest state is durable.
                    if let Err(e) = crate::store::control::save(&circle_dir, &state.control) {
                        warn!("[control] shutdown save failed: {e}");
                    }
                    break;
                }
                _ = interval.tick() => {
                    if let Err(e) = crate::store::control::save(&circle_dir, &state.control) {
                        warn!("[control] periodic save failed: {e}");
                    }
                }
            }
        }
    });
}

/// Whether a P2P change to our own member-list entry means we have been approved.
///
/// Must accept `Updated` as well as `Inserted`. A joining device writes a
/// provisional self-signed member entry for itself at startup (see the
/// auto-register block in `spawn_circle`), so when the admin's approval arrives
/// it lands on a key that already exists — an `Updated` change, never an
/// `Inserted` one. Matching only `Inserted` meant the approval was never
/// noticed, the device's pending entry was never cleared, and because that
/// entry lives in the shared CRDT it synced back and resurrected "awaiting
/// approval" on the admin too. Deterministic, not a race.
fn approval_clears_pending(change: &yrs::types::EntryChange) -> bool {
    matches!(
        change,
        yrs::types::EntryChange::Inserted(_) | yrs::types::EntryChange::Updated(_, _)
    )
}

/// Clear a peer's pending entry, retrying while the control doc is busy.
///
/// These removals are spawned from inside a Yjs observer, and an observer runs
/// while the transaction that triggered it still holds the control doc. A
/// single `try_transact_mut` therefore races that transaction's drop and loses
/// essentially every time — it had never once succeeded in practice, which
/// leaves a peer showing as "awaiting approval" in the UI forever despite
/// already being a full member.
async fn remove_pending_entry(state: &AppState, peer_str: &str, reason: &str) {
    for attempt in 0..PENDING_REMOVE_RETRIES {
        let removed = {
            match yrs::Transact::try_transact_mut(&*state.control) {
                Ok(mut txn) => {
                    use yrs::{Map, WriteTxn};
                    let pending = txn.get_or_insert_map(MLS_PENDING_KEY);
                    pending.remove(&mut txn, peer_str);
                    true
                }
                Err(_) => false,
            }
        };
        if removed {
            info!("[member] removed pending entry for {peer_str} ({reason})");
            return;
        }
        tokio::time::sleep(PENDING_REMOVE_BACKOFF * (attempt + 1)).await;
    }
    warn!("[member] control doc busy after retries; {peer_str} still shows as pending ({reason})");
}

/// Retry budget for taking the control document while auto-approving.
/// Nothing irreversible happens until both locks are held.
const APPROVAL_RETRIES: u32 = 10;
const APPROVAL_BACKOFF: std::time::Duration = std::time::Duration::from_millis(50);

async fn auto_approve(peer_id_str: String, state: AppState, mls: crate::mls::SharedMlsState) {
    use yrs::{Any, Map, Out, ReadTxn, Transact, WriteTxn};

    // One unit of work, for the reason spelled out in `api::members::approve_member`:
    // `add_member` advances the MLS epoch irreversibly, and the commit it returns
    // is the only way other devices can follow. Publishing that commit in a
    // separate transaction meant a momentarily busy control document silently
    // stranded every peer on the old epoch — silently, because each step here
    // simply returned.
    //
    // Take the MLS lock and the control-document write transaction together and
    // only then touch the group. Busy document: release both, retry, nothing has
    // happened. Both held: no await until the writes commit.
    let mut attempt: u32 = 0;
    loop {
        {
            let mut mls_locked = mls.lock().await;
            if let Ok(mut txn) = state.control.try_transact_mut() {
                let Some(kp_hex) = txn
                    .get_map(MLS_KEY_PACKAGES_KEY)
                    .and_then(|kp_map| kp_map.get(&txn, peer_id_str.as_str()))
                    .and_then(|v| match v {
                        Out::Any(Any::String(s)) => Some(s.to_string()),
                        _ => None,
                    })
                else {
                    return;
                };
                let Ok(kp_bytes) = hex::decode(&kp_hex) else {
                    return;
                };

                let (commit_bytes, welcome_bytes, ratchet_tree_bytes) = match mls_locked
                    .add_member(&kp_bytes)
                {
                    Ok(t) => t,
                    Err(_) => {
                        // Most likely already in the MLS group — e.g. the daemon
                        // restarted and re-wrote a pending entry before sync caught
                        // up. Retire the stale request so the UI stops showing it.
                        let already_member = matches!(
                            txn.get_map(MEMBER_LIST_KEY)
                                .and_then(|m| m.get(&txn, peer_id_str.as_str())),
                            Some(Out::Any(Any::String(_)))
                        );
                        if already_member {
                            let pending_map = txn.get_or_insert_map(MLS_PENDING_KEY);
                            pending_map.remove(&mut txn, peer_id_str.as_str());
                            info!(
                                    "[member] removed stale pending entry for {peer_id_str} (already a member)"
                                );
                        }
                        return;
                    }
                };
                let epoch = mls_locked.current_epoch().unwrap_or(0);

                let (owner, agent_id, device_label, agents) = txn
                    .get_map(MLS_PENDING_KEY)
                    .and_then(|pending_map| pending_map.get(&txn, peer_id_str.as_str()))
                    .and_then(|v| match v {
                        Out::Any(Any::String(s)) => serde_json::from_str::<PendingEntry>(&s)
                            .ok()
                            .map(|p| (p.owner, p.agent_id, p.device_label, p.agents)),
                        _ => None,
                    })
                    .unwrap_or_default();

                let commit_entry = MlsCommitEntry {
                    epoch,
                    data_hex: hex::encode(&commit_bytes),
                    sender_peer_id: state.peer_id.clone(),
                    ratchet_tree_hex: hex::encode(&ratchet_tree_bytes),
                };
                let member_entry = MemberEntry {
                    peer_id: peer_id_str.clone(),
                    owner: owner.clone(),
                    agent_id,
                    device_label,
                    agents,
                    role: MemberRole::Member,
                    added_at: chrono::Utc::now(),
                    signature: format!("add:{peer_id_str}:member:owner:{owner}"),
                };
                // Serialize before writing anything: skipping the commit while
                // still recording the member is the divergence this guards.
                let (Ok(commit_json), Ok(member_json)) = (
                    serde_json::to_string(&commit_entry),
                    serde_json::to_string(&member_entry),
                ) else {
                    warn!(
                        "[member] MLS group advanced for {peer_id_str} but its commit could not be serialized"
                    );
                    return;
                };

                let welcomes_map = txn.get_or_insert_map(MLS_WELCOMES_KEY);
                welcomes_map.insert(
                    &mut txn,
                    peer_id_str.as_str(),
                    hex::encode(&welcome_bytes).as_str(),
                );
                let commits_arr = txn.get_or_insert_array(MLS_COMMITS_KEY);
                commits_arr.push_back(&mut txn, commit_json.as_str());
                let member_map = txn.get_or_insert_map(MEMBER_LIST_KEY);
                member_map.insert(&mut txn, peer_id_str.as_str(), member_json.as_str());
                let pending_map = txn.get_or_insert_map(MLS_PENDING_KEY);
                pending_map.remove(&mut txn, peer_id_str.as_str());
                drop(txn);

                if let Err(e) = mls_locked.save(&state.circle_dir) {
                    tracing::error!(
                        "[member] approved {peer_id_str} but failed to persist the MLS group: {e}"
                    );
                }
                break;
            }
        }
        attempt += 1;
        if attempt >= APPROVAL_RETRIES {
            warn!("[member] control doc busy after retries; {peer_id_str} not approved this pass");
            return;
        }
        tokio::time::sleep(APPROVAL_BACKOFF * attempt).await;
    }

    let _ = state.events.send(crate::control::CircleEvent::MemberAdded {
        peer_id: peer_id_str,
    });
}

// ── PSK rotation helpers ──────────────────────────────────────────────────────

/// Called by the joiner when mls_welcomes[our_peer_id] arrives via P2P sync.
pub(crate) async fn consume_welcome(welcome_hex: String, mls: SharedMlsState, state: AppState) {
    let welcome_bytes = match hex::decode(&welcome_hex) {
        Ok(b) => b,
        Err(_) => return,
    };

    // Join the MLS group and persist it. We deliberately do NOT derive an epoch
    // PSK or rotate the transport key here: the transport PSK is a stable
    // per-circle network gate, and eviction is enforced by the mls_removed
    // sync-gate (see docs/concepts/security.md). MLS membership is still tracked for
    // the sync gate and content-layer encryption.
    let mut mls_locked = mls.lock().await;
    // Skip if we already joined (race: observer fires twice).
    if mls_locked.group.is_some() {
        return;
    }
    let identity_ptr = &mls_locked.identity as *const MlsIdentity;
    let identity = unsafe { &*identity_ptr };
    // ratchet_tree_bytes is None because use_ratchet_tree_extension is enabled —
    // the ratchet tree is embedded inside the Welcome bytes.
    let group = match MlsGroupManager::join_from_welcome(identity, &welcome_bytes, None) {
        Ok(g) => g,
        Err(e) => {
            warn!("[mls] join_from_welcome failed: {e}");
            return;
        }
    };
    let _ = group.save(identity, &state.circle_dir);
    mls_locked.group = Some(group);
    let _ = mls_locked.refresh_content_secret();
    info!("[mls] joined group via Welcome (membership tracked; transport PSK stays stable)");
}

/// Called for every new MlsCommitEntry that arrives from a peer.
/// Skips commits already applied (post-commit epoch <= current), skips our own commits,
/// and does nothing if the group becomes inactive (we were removed — we'll
/// be locked out naturally when others rotate to the new PSK).
pub(crate) async fn apply_commit_entry(
    entry: MlsCommitEntry,
    mls: SharedMlsState,
    state: AppState,
) {
    // Don't apply commits we ourselves produced.
    if entry.sender_peer_id == state.peer_id {
        return;
    }

    let commit_bytes = match hex::decode(&entry.data_hex) {
        Ok(b) => b,
        Err(_) => return,
    };

    // Apply the commit to keep our MLS group state in sync (epoch advances are
    // tracked for the sync gate and content encryption). We do NOT derive
    // an epoch PSK or rotate the transport key — the transport PSK is a stable
    // per-circle gate and eviction is the mls_removed sync-gate. See
    // docs/concepts/security.md.
    let mut mls_locked = mls.lock().await;
    // Take raw pointer to identity before the mutable group borrow.
    let identity_ptr = &mls_locked.identity as *const MlsIdentity;
    let identity = unsafe { &*identity_ptr };
    let group = match mls_locked.group.as_mut() {
        Some(g) => g,
        None => return, // not in group yet — will consume via Welcome path
    };
    let current_epoch = group.epoch();
    // Entries record the post-commit epoch. Skip if we already reached it.
    if entry.epoch <= current_epoch {
        return;
    }

    match group.apply_commit(identity, &commit_bytes) {
        Ok(()) => {
            let _ = group.save(identity, &state.circle_dir);
            info!(
                "[mls] applied Commit epoch {} → {} (membership tracked)",
                entry.epoch,
                group.epoch()
            );
            let _ = mls_locked.refresh_content_secret();
        }
        Err(e) => {
            warn!("[mls] apply_commit (epoch {}): {e}", entry.epoch);
        }
    }
}

/// Saves the new PSK to config.toml, then stops and restarts the circle so
/// the swarm rebuilds its transport with the new pnet key.
///
/// NOTE: no longer wired to MLS epoch changes — the transport PSK is a stable
/// per-circle gate and eviction is the mls_removed sync-gate (see
/// docs/concepts/security.md). Retained for a possible future *explicit* circle-key
/// rotation (e.g. a manual `enox rotate-key` admin action); currently unused.
#[allow(dead_code)]
pub async fn rotate_psk_and_restart(circle_id: &str, new_psk: [u8; 32], daemon: DaemonState) {
    let mut cfg = match config::load(circle_id) {
        Ok(c) => c,
        Err(e) => {
            warn!("[mls] rotate_psk: config load failed: {e}");
            return;
        }
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

/// Re-publish this device's advertised agents / label into every active
/// circle's member list, so a change to `agents.toml` (e.g. an agent added via
/// the settings API) becomes visible to peers without a daemon restart.
///
/// Startup already syncs the self-entry once during join (see the refresh block
/// in `spawn_circle`); this is the same update triggered on demand. It is a
/// no-op for any circle where we don't yet have a member entry (still pending),
/// and for entries already in sync.
pub fn readvertise_local_agents(daemon: &DaemonState) {
    use yrs::{Any, Map, Out, ReadTxn, Transact, WriteTxn};

    let current_agents = crate::identity::read_local_agents();
    let current_label = crate::identity::read_identity_display()
        .map(|(label, _)| label)
        .unwrap_or_default();

    for state in daemon.list() {
        let self_key = state.peer_id.clone();

        let existing: Option<MemberEntry> = {
            let Ok(txn) = state.control.try_transact() else {
                continue;
            };
            match txn
                .get_map(MEMBER_LIST_KEY)
                .and_then(|map| map.get(&txn, self_key.as_str()))
            {
                Some(Out::Any(Any::String(s))) => serde_json::from_str(&s).ok(),
                _ => None,
            }
        };

        if let Some(mut entry) = existing {
            if entry.agents != current_agents || entry.device_label != current_label {
                entry.agents = current_agents.clone();
                entry.device_label = current_label.clone();
                if let Ok(json_str) = serde_json::to_string(&entry) {
                    {
                        let Ok(mut txn) = state.control.try_transact_mut() else {
                            continue;
                        };
                        let map = txn.get_or_insert_map(MEMBER_LIST_KEY);
                        map.insert(&mut txn, self_key.as_str(), json_str.as_str());
                    }
                    // Nudge subscribers (incl. this device's own chat stream) to
                    // re-fetch the roster so mention pickers show the new agent.
                    let _ = state.events.send(crate::control::CircleEvent::MemberAdded {
                        peer_id: self_key.clone(),
                    });
                    info!(
                        "[member] re-advertised agents/label for self in {}",
                        state.circle_id
                    );
                }
            }
        }
    }
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

    if addr.to_string().contains("p2p-circuit") {
        return false;
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::MLS_PENDING_KEY;
    use crate::state::AppState;
    use yrs::{Map, ReadTxn, Transact, WriteTxn};

    fn test_state() -> AppState {
        AppState::new(
            "circle".into(),
            "Circle".into(),
            std::path::PathBuf::new(),
            std::path::PathBuf::new(),
            String::new(),
            "agent".into(),
            1,
            "peer-local".into(),
            crate::config::JoinPolicy::Manual,
            "owner".into(),
            crate::mls::new_mls_state(
                crate::mls::MlsIdentity::generate("peer-local").unwrap(),
                None,
            ),
        )
    }

    fn seed_pending(state: &AppState, peer: &str) {
        let mut txn = state.control.try_transact_mut().unwrap();
        let pending = txn.get_or_insert_map(MLS_PENDING_KEY);
        pending.insert(&mut txn, peer, "{}");
    }

    fn is_pending(state: &AppState, peer: &str) -> bool {
        let txn = state.control.try_transact().unwrap();
        txn.get_map(MLS_PENDING_KEY)
            .and_then(|pending| pending.get(&txn, peer))
            .is_some()
    }

    /// Regression: a joining device writes a provisional self-signed member
    /// entry for itself, so the admin's approval arrives as an update to an
    /// existing key rather than an insert. Matching only `Inserted` meant the
    /// approval was never noticed and every fresh join left a permanent
    /// "awaiting approval" that synced back to the admin.
    #[test]
    fn approval_is_recognised_whether_inserted_or_updated() {
        use yrs::types::EntryChange;
        use yrs::{Any, Out};

        let value = || Out::Any(Any::String("{}".into()));

        assert!(
            approval_clears_pending(&EntryChange::Inserted(value())),
            "a first-time write of our member entry is an approval"
        );
        assert!(
            approval_clears_pending(&EntryChange::Updated(value(), value())),
            "an approval landing on our provisional self-written entry still counts"
        );
        assert!(
            !approval_clears_pending(&EntryChange::Removed(value())),
            "removal from the member list is not an approval"
        );
    }

    /// Regression: clearing a pending entry is spawned from inside a Yjs
    /// observer, which runs while the triggering transaction still holds the
    /// control doc. A single `try_transact_mut` loses that race essentially
    /// every time, which left an approved peer showing as "awaiting approval"
    /// in the UI forever. The removal must outlive the contention.
    #[tokio::test]
    async fn pending_entry_removal_outlives_contention() {
        let state = test_state();
        let peer = "12D3KooWtest";
        seed_pending(&state, peer);

        let task = {
            let s = state.clone();
            let peer = peer.to_string();
            tokio::spawn(async move { remove_pending_entry(&s, &peer, "test").await })
        };

        // Hold the control doc while the removal is already running, the way
        // the observer's own transaction does.
        {
            let _held = state.control.try_transact_mut().unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        }

        task.await.unwrap();
        assert!(
            !is_pending(&state, peer),
            "pending entry must be cleared once the control doc frees up"
        );
    }
}
