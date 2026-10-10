//! Per-circle spawn logic — called at daemon startup, on hot-reload, and from the
//! `POST /circles/<id>/start` API endpoint.

use anyhow::Result;
use libp2p_identity::PeerId;
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Retry budget for clearing a peer's pending entry. See [`remove_pending_entry`].
const PENDING_REMOVE_RETRIES: u32 = 10;
const PENDING_REMOVE_BACKOFF: std::time::Duration = std::time::Duration::from_millis(50);

/// How often the swarm loop sweeps for connections it needs to rebuild.
/// Bounds how long a circle stays split after a relayed circuit hits the
/// relay's duration or byte cap.
pub(crate) const RECONNECT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
/// Cap on the backoff exponent, so a peer that stays offline is still retried
/// every `RECONNECT_INTERVAL * 2^RECONNECT_MAX_BACKOFF_EXP` (30s * 16 = 8 min).
const RECONNECT_MAX_BACKOFF_EXP: u32 = 4;

use yrs::{Array, Observable};

use crate::{
    config::{self, CircleConfig, JoinPolicy},
    control::{
        MemberEntry, MemberRole, MlsCommitEntry, OwnerClaim, PendingEntry, INVITE_NONCES_KEY,
        MEMBER_LIST_KEY, MLS_COMMITS_KEY, MLS_KEY_PACKAGES_KEY, MLS_KEY_PACKAGE_BINDINGS_KEY,
        MLS_OWNER_CLAIMS_KEY, MLS_PENDING_KEY, MLS_WELCOMES_KEY,
    },
    crypto::{keypair_from_hex, psk_from_hex},
    daemon::DaemonState,
    mls::{MlsGroupManager, MlsIdentity, SharedMlsState},
    presence,
    state::AppState,
    sync_yjs::watcher::spawn_watcher,
};

pub async fn spawn_circle(config: CircleConfig, daemon: DaemonState) -> Result<()> {
    let keypair = keypair_from_hex(&config.keypair_proto_hex)?;
    let peer_id = keypair.public().to_peer_id();
    let psk_bytes = psk_from_hex(&config.psk_hex)?;

    let workspace = if config.workspace_dir.is_empty() {
        crate::config::circle_dir(&config.circle_id)?.join("files")
    } else {
        std::path::PathBuf::from(&config.workspace_dir)
    };
    let workspace = crate::config::normalize_workspace_dir(&workspace)?;
    // Claim the directory before doing any loading. Scanning the active set
    // instead only worked while circles started one at a time: a circle is
    // absent from that set until it has finished, so two circles sharing a
    // workspace could both find it unclaimed and both start watching and
    // syncing the same files.
    let workspace_claim = daemon
        .claim_workspace(crate::config::workspace_key(&workspace)?, &config.circle_id)
        .map_err(|owner| {
            let name = daemon
                .get(&owner)
                .map(|active| active.circle_name.clone())
                .unwrap_or_else(|| owner.clone());
            anyhow::anyhow!(
                "workspace {} is already active in circle '{}' ({})",
                workspace.display(),
                name,
                owner
            )
        })?;
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

    // Publish MLS key package, with this peer's signature binding its MLS
    // signature key to its peer id.
    {
        use yrs::{Map, Transact};
        let (kp_bytes, binding) = {
            let mls_locked = mls.lock().await;
            let kp = mls_locked.identity.generate_key_package()?;
            let binding = crate::identity::sign_key_package_binding(
                &keypair,
                &config.circle_id,
                mls_locked.identity.signer.public(),
            )?;
            (kp, binding)
        };
        let kp_hex = hex::encode(&kp_bytes);
        let kp_map = state.control.get_or_insert_map(MLS_KEY_PACKAGES_KEY);
        let bindings_map = state
            .control
            .get_or_insert_map(MLS_KEY_PACKAGE_BINDINGS_KEY);
        let mut txn = state.control.transact_mut();
        kp_map.insert(&mut txn, peer_id.to_string().as_str(), kp_hex.as_str());
        bindings_map.insert(&mut txn, peer_id.to_string().as_str(), binding.as_str());
    }

    // Sign and publish owner claim
    {
        use yrs::{Map, Transact};
        let owner_claim_msg = format!("owner:{}", config.owner);
        let owner_sig = keypair
            .sign(owner_claim_msg.as_bytes())
            .map(hex::encode)
            .unwrap_or_default();
        // What turns the name into something checkable: the device this circle
        // key was derived from, a signature tying the two together, and the
        // chain carrying the user root's authority down to that device. A
        // device that has not been linked to a user publishes the name alone,
        // exactly as before, and peers will read it as unproven.
        let device = crate::identity::DeviceIdentity::load().ok();
        let (user_pubkey_hex, device_pubkey_hex, device_binding_hex, attestation_chain) =
            match device {
                Some(ref d) if d.attestation_is_valid() => {
                    // Signed with the *device* key over this circle key: the
                    // device authorising the peer, not the peer claiming the
                    // device. See `identity::binding_message` for why the other
                    // direction is a spoof anyone in the circle can mount.
                    let binding = d.device_keypair().ok().and_then(|dk| {
                        crate::identity::sign_binding(&dk, &config.circle_id, &keypair).ok()
                    });
                    (
                        d.user_pubkey_hex.clone(),
                        d.device_pubkey_hex().ok(),
                        binding,
                        d.attestation_chain.clone(),
                    )
                }
                _ => (None, None, None, Vec::new()),
            };

        let claim = OwnerClaim {
            owner: config.owner.clone(),
            sig: owner_sig,
            user_pubkey_hex,
            device_pubkey_hex,
            device_binding_hex,
            attestation_chain,
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
                ambient_agents: Vec::new(),
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
                    join_grant: config.join_grant.clone(),
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

    // Sweep immediately (including restored requests) and periodically. Pending
    // entries and key packages can arrive separately or be updated in place.
    //
    // Checked per tick, not once at start: a member can become admin while
    // running, when a leaving admin hands it the key.
    if config.join_policy == JoinPolicy::Auto {
        let approval_state = state.clone();
        let approval_token = token.clone();
        let admin_key = cdir.join("admin.key");
        tokio::spawn(async move {
            let mut ticks = tokio::time::interval(std::time::Duration::from_secs(5));
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = approval_token.cancelled() => break,
                    _ = ticks.tick() => {
                        if admin_key.exists() {
                            retry_pending_approvals(&approval_state).await;
                        }
                    }
                }
            }
        });
    }

    // ── Upgrade check ────────────────────────────────────────────────────────
    // Ask the Circle's bootstrap server whether it still serves this version,
    // so a transport change old clients cannot follow is reported, not silent.
    {
        let notice_state = state.clone();
        let notice_token = token.clone();
        let hosts = crate::upgrade_check::hosts(&config.rendezvous_addrs);
        let cid = config.circle_id.clone();
        tokio::spawn(async move {
            let mut ticks = tokio::time::interval(crate::upgrade_check::INTERVAL);
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = notice_token.cancelled() => break,
                    _ = ticks.tick() => {
                        let notice = crate::upgrade_check::check(&hosts).await;
                        if let Some(n) = &notice {
                            warn!(
                                "[{cid}] {} requires enox {} or newer (this is {}); upgrade with `enox update`",
                                n.server, n.min_version, crate::version::VERSION
                            );
                        }
                        *notice_state.upgrade_notice.write().unwrap() = notice;
                    }
                }
            }
        });
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

    // Before anything opens a store. A store that creates its directory first
    // would leave the migration with nowhere to move the old one to, so this
    // stays ahead of the watcher and the proposal/event stores below rather
    // than relying on their internal ordering.
    let moved = crate::store::layout::migrate_legacy_layout(&state.workspace);
    if moved > 0 {
        info!("[layout] consolidated {moved} legacy state director(ies) under .enox/");
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
    if let Err(error) = crate::proposal::runs::migrate_legacy(&state.circle_dir) {
        warn!(
            "[{}] managed run migration failed: {error}",
            state.circle_id
        );
    }
    crate::proposal::engine::spawn_engine(state.clone(), token.clone());
    crate::agent::reaction::spawn_reaction(state.clone(), token.clone());
    presence::spawn_presence(state.clone(), agent_id, token.clone());
    spawn_control_persist(state.clone(), cdir.clone(), token.clone());
    spawn_storage_gc(state.clone(), token.clone());

    // ── Network: one Iroh endpoint for this Circle ──────────────────────────
    let started =
        crate::network::iroh_net::spawn(&config, state.clone(), &keypair, psk_bytes, token.clone())
            .await;
    if let Err(error) = started {
        // Everything above is already running; stop it, or the daemon's retry
        // starts a second copy alongside it.
        token.cancel();
        return Err(error);
    }

    daemon.insert_circle(config.circle_id.clone(), state, token);
    // The circle is live; the claim now belongs to it until `stop_circle`.
    workspace_claim.retain();
    Ok(())
}

/// Periodically reclaim storage that nothing refers to any more.
///
/// Runs on a long interval rather than on every change: collection walks the
/// blob directory, and the space it reclaims is not urgent. The first pass is
/// delayed so it never competes with startup — a device that has just come
/// online is busy reconciling, and that is also when blobs are most likely to
/// be about to become reachable again.
fn spawn_storage_gc(state: AppState, token: CancellationToken) {
    tokio::spawn(async move {
        // Six hours: often enough that storage cannot run away, rare enough to
        // be invisible.
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(6 * 60 * 60));
        interval.tick().await; // skip the immediate first tick
        loop {
            tokio::select! {
                _ = token.cancelled() => break,
                _ = interval.tick() => {
                    let workspace = state.workspace.clone();
                    let proposal_report = tokio::task::spawn_blocking(move || {
                        let store = crate::proposal::store::ProposalStore::open(&workspace)?;
                        crate::proposal::gc::collect(&store)
                    })
                    .await;
                    match proposal_report {
                        Ok(Ok(report)) if !report.is_empty() => info!(
                            "[gc] reclaimed {} proposal(s), {} snapshot(s), {} blob(s) ({:.1} MB)",
                            report.proposals_removed,
                            report.snapshots_removed,
                            report.blobs_removed,
                            report.bytes_reclaimed as f64 / 1_048_576.0,
                        ),
                        Ok(Ok(_)) => {}
                        Ok(Err(e)) => warn!("[gc] proposal collection failed: {e}"),
                        Err(e) => warn!("[gc] proposal collection panicked: {e}"),
                    }

                    match crate::api::attachments::collect_unreferenced_blobs(&state) {
                        Ok((0, _)) => {}
                        Ok((count, bytes)) => info!(
                            "[gc] reclaimed {count} chat attachment(s) ({:.1} MB)",
                            bytes as f64 / 1_048_576.0,
                        ),
                        Err(e) => warn!("[gc] attachment collection failed: {e}"),
                    }
                }
            }
        }
    });
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

/// Whether an invite grant entitles its bearer to be admitted right now.
///
/// Authorship alone is not enough. A grant proves which member issued the
/// invite; this decides whether that member is *still* entitled to admit
/// anyone, which is re-evaluated at the moment the invite is redeemed rather
/// than when it was written. Removing a member therefore invalidates every
/// invite they ever issued, without rotating any shared secret and without
/// narrowing who may invite.
///
/// Reads from the caller's transaction so the whole admission decision sees one
/// consistent view of membership.
/// The distrusted user identity this peer proves, if it proves one.
///
/// Only a peer that *proves* an identity can be caught here: a device that
/// publishes no owner claim has no identity to match against. That is the
/// current posture rather than a hole — nothing yet requires a joiner to prove
/// who it is, so distrust disowns an identity rather than screening every
/// stranger. Requiring proof is the step after this one.
pub(crate) fn distrusted_identity<T: yrs::ReadTxn>(
    txn: &T,
    circle_id: &str,
    admin_pubkey_hex: &str,
    peer_id: &str,
) -> Option<String> {
    use yrs::{Any, Map, Out};

    let distrusted = txn.get_map(crate::control::DISTRUSTED_USERS_KEY)?;
    let claim = txn
        .get_map(MLS_OWNER_CLAIMS_KEY)
        .and_then(|m| m.get(txn, peer_id))
        .and_then(|v| match v {
            Out::Any(Any::String(s)) => serde_json::from_str::<OwnerClaim>(&s).ok(),
            _ => None,
        })?;

    let user = claim.verified_user(peer_id, circle_id)?;
    let entry = distrusted.get(txn, user.as_str()).and_then(|v| match v {
        Out::Any(Any::String(s)) => serde_json::from_str::<crate::control::DistrustEntry>(&s).ok(),
        _ => None,
    })?;

    // The record has to carry the admin's signature, not merely exist. The
    // control document is replicated and every member can write to it, so
    // treating presence as authority would let any member lock anybody out of
    // the circle by adding an entry of their own.
    if !entry.is_authentic(admin_pubkey_hex) {
        warn!(
            "[member] ignoring an unsigned distrust record for {}",
            &user[..user.len().min(16)]
        );
        return None;
    }
    Some(user)
}

fn grant_admits<T: yrs::ReadTxn>(
    txn: &T,
    circle_id: &str,
    peer_id: &str,
    grant: Option<&crate::control::JoinGrant>,
) -> Result<(), String> {
    use crate::control::MLS_REMOVED_KEY;
    use yrs::{Any, Map, Out};

    let Some(grant) = grant else {
        return Err("no invite grant presented".into());
    };

    if chrono::Utc::now() > grant.expires_at {
        return Err(format!("invite expired at {}", grant.expires_at));
    }

    let invite_grant = crate::invite::InviteGrant {
        inviter_pubkey_hex: grant.inviter_pubkey_hex.clone(),
        nonce: grant.nonce.clone(),
        sig: grant.sig.clone(),
    };
    if let Err(e) = crate::invite::verify_grant(circle_id, &invite_grant, grant.expires_at) {
        return Err(format!("invalid invite grant: {e}"));
    }

    let inviter = invite_grant
        .inviter_peer_id()
        .map_err(|e| format!("invite grant names no usable issuer: {e}"))?;

    // An invite admits one device. Without this, an invite that legitimately
    // admitted someone once would keep working.
    let already_used = txn
        .get_map(INVITE_NONCES_KEY)
        .and_then(|used| used.get(txn, grant.nonce.as_str()))
        .is_some();
    if already_used {
        return Err("invite already redeemed".into());
    }

    // The issuer must still be a member in good standing. This is the check the
    // whole scheme rests on.
    if txn
        .get_map(MLS_REMOVED_KEY)
        .and_then(|removed| removed.get(txn, inviter.as_str()))
        .is_some()
    {
        return Err(format!("invite was issued by removed member {inviter}"));
    }
    let inviter_is_member = matches!(
        txn.get_map(MEMBER_LIST_KEY)
            .and_then(|members| members.get(txn, inviter.as_str())),
        Some(Out::Any(Any::String(_)))
    );
    if !inviter_is_member {
        return Err(format!("invite was issued by non-member {inviter}"));
    }

    let _ = peer_id;
    Ok(())
}

/// Retry budget for taking the control document while auto-approving.
/// Nothing irreversible happens until both locks are held.
const APPROVAL_RETRIES: u32 = 10;
const APPROVAL_BACKOFF: std::time::Duration = std::time::Duration::from_millis(50);

/// Why a join request from a member with a leaf is left waiting.
pub(crate) const UNPROVEN_REJOIN: &str = "This device already has a place in the circle, and its \
     new key is not signed by it or not newer than that place. If it rejoined, restart it \
     so it publishes a fresh key.";

/// The binding `peer_id` published for its KeyPackage, if any.
pub(crate) fn key_package_binding<T: yrs::ReadTxn>(txn: &T, peer_id: &str) -> Option<String> {
    use yrs::{Any, Map, Out};
    txn.get_map(MLS_KEY_PACKAGE_BINDINGS_KEY)
        .and_then(|bindings| bindings.get(txn, peer_id))
        .and_then(|v| match v {
            Out::Any(Any::String(s)) => Some(s.to_string()),
            _ => None,
        })
}

/// Whether `binding` shows `peer_id` itself vouching for MLS `signature_key`.
pub(crate) fn binding_proves(
    circle_id: &str,
    peer_id: &str,
    signature_key: &[u8],
    binding: Option<&str>,
) -> bool {
    binding.is_some_and(|binding| {
        crate::identity::verify_key_package_binding(peer_id, circle_id, signature_key, binding)
    })
}

async fn retry_pending_approvals(state: &AppState) {
    use yrs::{Map, ReadTxn, Transact};
    let peers: Vec<String> = {
        let Ok(txn) = state.control.try_transact() else {
            return;
        };
        txn.get_map(MLS_PENDING_KEY)
            .map(|pending| {
                pending
                    .iter(&txn)
                    .map(|(peer, _)| peer.to_string())
                    .collect()
            })
            .unwrap_or_default()
    };
    state.approval_errors.retain(|peer, _| peers.contains(peer));
    for peer in peers {
        auto_approve(peer, state.clone(), state.mls.clone()).await;
    }
}

fn record_approval_error(state: &AppState, peer: &str, reason: String) {
    if state.approval_errors.get(peer).as_deref() != Some(&reason) {
        warn!("[member] automatic approval of {peer} failed: {reason}");
        state.approval_errors.insert(peer.to_owned(), reason);
    }
}

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
                // Another attempt or a manual decision may already have finished.
                let pending = txn.get_or_insert_map(MLS_PENDING_KEY);
                if pending.get(&txn, peer_id_str.as_str()).is_none() {
                    state.approval_errors.remove(&peer_id_str);
                    return;
                }
                let kp_hex = txn
                    .get_map(MLS_KEY_PACKAGES_KEY)
                    .and_then(|kp_map| kp_map.get(&txn, peer_id_str.as_str()))
                    .and_then(|v| match v {
                        Out::Any(Any::String(s)) => Some(s.to_string()),
                        _ => None,
                    });
                // The coordination member list includes provisional self-entries;
                // only a leaf holding the keys this device publishes proves
                // completed admission. A leaf holding other keys is left over
                // from before the device left and entered again — readmit it.
                let published = kp_hex.as_deref().and_then(|h| hex::decode(h).ok());
                let binding = key_package_binding(&txn, &peer_id_str);
                match mls_locked.standing(&peer_id_str, published.as_deref(), |key| {
                    binding_proves(&state.circle_id, &peer_id_str, key, binding.as_deref())
                }) {
                    crate::mls::Standing::Current => {
                        pending.remove(&mut txn, peer_id_str.as_str());
                        state.approval_errors.remove(&peer_id_str);
                        return;
                    }
                    // Not admission, so the request stays: the device's next
                    // start publishes a KeyPackage that can replace the leaf.
                    crate::mls::Standing::Unproven => {
                        record_approval_error(&state, &peer_id_str, UNPROVEN_REJOIN.into());
                        return;
                    }
                    crate::mls::Standing::Absent | crate::mls::Standing::Stale => {}
                }
                let Some(kp_hex) = kp_hex else {
                    record_approval_error(
                        &state,
                        &peer_id_str,
                        "Waiting for device key package".into(),
                    );
                    return;
                };
                let Ok(kp_bytes) = hex::decode(&kp_hex) else {
                    record_approval_error(
                        &state,
                        &peer_id_str,
                        "Invalid device key package encoding".into(),
                    );
                    return;
                };

                // Decide admission before touching the group. `add_member`
                // advances the epoch irreversibly and issues a Welcome to it,
                // so anything that should refuse a joiner has to refuse here.
                let presented = txn
                    .get_map(MLS_PENDING_KEY)
                    .and_then(|pending| pending.get(&txn, peer_id_str.as_str()))
                    .and_then(|v| match v {
                        Out::Any(Any::String(s)) => serde_json::from_str::<PendingEntry>(&s).ok(),
                        _ => None,
                    })
                    .and_then(|entry| entry.join_grant);
                // A circle that has disowned an identity must not readmit it,
                // including on a device minted after the fact — which is the
                // whole point, since whoever holds a stolen root key can make
                // as many devices as they like.
                if let Some(user) = distrusted_identity(
                    &txn,
                    &state.circle_id,
                    &state.admin_pubkey_hex,
                    &peer_id_str,
                ) {
                    record_approval_error(
                        &state,
                        &peer_id_str,
                        format!(
                            "User identity {} is distrusted in this circle",
                            &user[..user.len().min(16)]
                        ),
                    );
                    return;
                }

                if let Err(reason) =
                    grant_admits(&txn, &state.circle_id, &peer_id_str, presented.as_ref())
                {
                    record_approval_error(&state, &peer_id_str, reason);
                    // Leave the request pending rather than discarding it: an
                    // admin can still approve deliberately, which is the right
                    // escape hatch for a legitimate joiner whose invite lapsed.
                    return;
                }

                let (commit_bytes, welcome_bytes, ratchet_tree_bytes) =
                    match mls_locked.admit_member(&peer_id_str, &kp_bytes) {
                        Ok(t) => t,
                        Err(error) => {
                            record_approval_error(
                                &state,
                                &peer_id_str,
                                format!("MLS admission failed: {error}"),
                            );
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
                    ambient_agents: Vec::new(),
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
                state.approval_errors.remove(&peer_id_str);
                // Burn the nonce in the same transaction that admits, so an
                // invite cannot admit twice even under a concurrent redemption.
                if let Some(grant) = presented.as_ref() {
                    let used = txn.get_or_insert_map(INVITE_NONCES_KEY);
                    used.insert(
                        &mut txn,
                        grant.nonce.as_str(),
                        chrono::Utc::now().to_rfc3339().as_str(),
                    );
                }
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
    // Which of them read the room is a per-Circle answer now, so it is
    // resolved inside the loop below rather than once for every Circle.
    let cfg = crate::agent::config::AgentConfig::load();
    let current_label = crate::identity::read_identity_display()
        .map(|(label, _)| label)
        .unwrap_or_default();

    for state in daemon.list() {
        let self_key = state.peer_id.clone();
        // Advertised so every peer can see who is listening here, not just the
        // device that opted in (§2.6).
        let current_ambient: Vec<String> = cfg
            .resolved(&state.circle_id)
            .ambient
            .into_iter()
            .filter(|name| cfg.agents.contains_key(name))
            .collect();

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
            if entry.agents != current_agents
                || entry.device_label != current_label
                || entry.ambient_agents != current_ambient
            {
                entry.agents = current_agents.clone();
                entry.ambient_agents = current_ambient.clone();
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

/// Peer ids in the circle roster, or an empty list if the control doc is busy
/// (the next sweep picks it up — never block the swarm loop on a lock).
pub(crate) fn member_peer_ids(state: &AppState) -> Vec<PeerId> {
    use yrs::{Map, ReadTxn, Transact};

    let Ok(txn) = state.control.try_transact() else {
        return Vec::new();
    };
    let Some(member_map) = txn.get_map(MEMBER_LIST_KEY) else {
        return Vec::new();
    };
    member_map
        .iter(&txn)
        .filter_map(|(peer_id, _)| peer_id.parse::<PeerId>().ok())
        .collect()
}

/// Mark `peer`'s agent offline in presence as soon as its last connection
/// closes, so every device sees it without waiting for the heartbeat to lapse.
pub(crate) fn mark_peer_offline(state: &AppState, peer: &str) {
    use yrs::{Any, Map, Out, ReadTxn, Transact};
    let Ok(txn) = state.control.try_transact() else {
        return;
    };
    let agent_id = txn
        .get_map(MEMBER_LIST_KEY)
        .and_then(|members| members.get(&txn, peer))
        .and_then(|value| match value {
            Out::Any(Any::String(s)) => serde_json::from_str::<MemberEntry>(&s)
                .ok()
                .map(|m| m.agent_id),
            _ => None,
        });
    drop(txn);
    if let Some(agent_id) = agent_id {
        presence::write_offline(state, &agent_id);
    }
}

/// One retry budget per peer across explicit discovery and reconnect paths.
/// Use elapsed time rather than sweep numbers: repeated discovery events must
/// not create extra attempts between ticks or reset the retry schedule.
#[derive(Default)]
pub(crate) struct PeerRedials {
    attempts: HashMap<PeerId, (u32, std::time::Instant)>,
    connected_since: HashMap<PeerId, std::time::Instant>,
}

impl PeerRedials {
    pub(crate) fn allow(&mut self, peer: PeerId, now: std::time::Instant) -> bool {
        let (failures, deadline) = self.attempts.entry(peer).or_insert((0, now));
        if now < *deadline {
            return false;
        }
        *deadline = now + RECONNECT_INTERVAL * (1 << (*failures).min(RECONNECT_MAX_BACKOFF_EXP));
        *failures = failures.saturating_add(1);
        true
    }

    pub(crate) fn connected(&mut self, peer: PeerId, now: std::time::Instant) {
        self.connected_since.entry(peer).or_insert(now);
    }

    pub(crate) fn disconnected(&mut self, peer: PeerId, now: std::time::Instant) {
        // Brief successes include rejected circle handshakes and duplicate
        // relay circuits. Only a sustained connection earns a fresh budget.
        if let Some(since) = self.connected_since.remove(&peer) {
            if now.duration_since(since) >= RECONNECT_INTERVAL * 2 {
                self.attempts.remove(&peer);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::MLS_PENDING_KEY;
    use crate::state::AppState;
    use yrs::{Map, ReadTxn, Transact, WriteTxn};

    #[test]
    fn discovery_and_sweeps_share_backoff_despite_brief_transport_success() {
        let peer = PeerId::random();
        let start = std::time::Instant::now();
        let mut retries = PeerRedials::default();
        let mut attempts = Vec::new();
        // A discovery arrives every second, with a periodic sweep too. Each
        // transport opens successfully then fails its handshake immediately.
        for second in 0..=1200 {
            let now = start + std::time::Duration::from_secs(second);
            if retries.allow(peer, now) {
                attempts.push(second);
                retries.connected(peer, now);
                retries.disconnected(peer, now + std::time::Duration::from_millis(100));
            }
            if second % 30 == 0 {
                assert!(
                    !retries.allow(peer, now),
                    "sweep must not bypass discovery's deadline"
                );
            }
        }
        assert_eq!(attempts, vec![0, 30, 90, 210, 450, 930]);
    }

    #[test]
    fn shared_ip_relay_budget_survives_discovery_churn_with_backoff() {
        // Exercise libp2p's actual token buckets with the production relay
        // config. Five circle identities share one IP, each discovering three
        // unreachable/briefly-connected peers once a second for five minutes.
        fn simulate(backoff: bool) -> (usize, usize) {
            let mut relay = crate::bootstrap::relay_server_config();
            let sources: Vec<_> = (0..5).map(|_| PeerId::random()).collect();
            let destinations: Vec<_> = (0..3).map(|_| PeerId::random()).collect();
            let mut retries: Vec<_> = (0..5).map(|_| PeerRedials::default()).collect();
            let addr: libp2p::Multiaddr = "/ip4/203.0.113.1/tcp/1234".parse().unwrap();
            let start = std::time::Instant::now();
            let (mut admitted, mut denied) = (0, 0);
            for second in 0..300 {
                let now = start + std::time::Duration::from_secs(second);
                for (i, source) in sources.iter().enumerate() {
                    for destination in &destinations {
                        if backoff && !retries[i].allow(*destination, now) {
                            continue;
                        }
                        if relay
                            .circuit_src_rate_limiters
                            .iter_mut()
                            .all(|limiter| limiter.try_next(*source, &addr, now))
                        {
                            admitted += 1;
                            retries[i].connected(*destination, now);
                            retries[i].disconnected(
                                *destination,
                                now + std::time::Duration::from_millis(100),
                            );
                        } else {
                            denied += 1;
                        }
                    }
                }
            }
            (admitted, denied)
        }
        let before = simulate(false); // discovery previously dialed without a budget
        let after = simulate(true);
        eprintln!("relay token-bucket simulation: before={before:?}, after={after:?}");
        assert!(
            before.1 > 0,
            "the old discovery path must reproduce resource denials"
        );
        assert_eq!(
            after,
            (60, 0),
            "backoff must retain retries without exhausting the relay"
        );
    }

    #[test]
    fn retry_deadlines_are_per_peer_and_relative_to_the_last_attempt() {
        let mut retries = PeerRedials::default();
        let start = std::time::Instant::now();
        let a = PeerId::random();
        let b = PeerId::random();
        assert!(retries.allow(a, start));
        assert!(!retries.allow(a, start + std::time::Duration::from_secs(29)));
        assert!(retries.allow(b, start + std::time::Duration::from_secs(29)));
        assert!(retries.allow(a, start + std::time::Duration::from_secs(30)));
        assert!(!retries.allow(a, start + std::time::Duration::from_secs(60)));
    }

    #[test]
    fn sustained_connection_resets_backoff_only_after_last_connection_closes() {
        let mut retries = PeerRedials::default();
        let peer = PeerId::random();
        let start = std::time::Instant::now();
        assert!(retries.allow(peer, start));
        assert!(retries.allow(peer, start + std::time::Duration::from_secs(30)));
        retries.connected(peer, start + std::time::Duration::from_secs(31));
        // An additional connection must not reset the original uptime.
        retries.connected(peer, start + std::time::Duration::from_secs(80));
        retries.disconnected(peer, start + std::time::Duration::from_secs(91));
        assert!(retries.allow(peer, start + std::time::Duration::from_secs(91)));
        assert!(retries.allow(peer, start + std::time::Duration::from_secs(121)));
    }

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

    async fn automatic_state() -> (AppState, tempfile::TempDir) {
        let mut state = test_state();
        let dir = tempfile::tempdir().unwrap();
        state.circle_dir = dir.path().to_path_buf();
        state.join_policy = JoinPolicy::Auto;
        let identity = crate::mls::MlsIdentity::generate("peer-local").unwrap();
        let group = crate::mls::MlsGroupManager::create(&identity).unwrap();
        state.mls = crate::mls::new_mls_state(identity, Some(group));
        (state, dir)
    }

    fn seed_valid_request(state: &AppState, peer: &str) {
        let issuer = libp2p_identity::Keypair::generate_ed25519();
        let expires_at = chrono::Utc::now() + chrono::Duration::hours(1);
        let grant = crate::invite::sign_grant(
            &state.circle_id,
            &hex::encode(issuer.to_protobuf_encoding().unwrap()),
            expires_at,
        )
        .unwrap();
        let entry = PendingEntry {
            peer_id: peer.into(),
            owner: "owner".into(),
            agent_id: peer.into(),
            device_label: String::new(),
            agents: vec![],
            owner_sig: String::new(),
            requested_at: chrono::Utc::now(),
            join_grant: Some(crate::control::JoinGrant {
                inviter_pubkey_hex: grant.inviter_pubkey_hex,
                nonce: grant.nonce,
                sig: grant.sig,
                expires_at,
            }),
        };
        let mut txn = state.control.transact_mut();
        let members = txn.get_or_insert_map(MEMBER_LIST_KEY);
        members.insert(&mut txn, issuer.public().to_peer_id().to_string(), "{}");
        let pending = txn.get_or_insert_map(MLS_PENDING_KEY);
        pending.insert(&mut txn, peer, serde_json::to_string(&entry).unwrap());
    }

    #[tokio::test]
    async fn automatic_sweep_retries_restored_request_when_key_package_arrives() {
        let (state, _dir) = automatic_state().await;
        let peer = "peer-joiner";
        seed_valid_request(&state, peer);
        retry_pending_approvals(&state).await;
        assert!(is_pending(&state, peer));
        assert!(state.approval_errors.contains_key(peer));
        let identity = crate::mls::MlsIdentity::generate(peer).unwrap();
        {
            let mut txn = state.control.transact_mut();
            let packages = txn.get_or_insert_map(MLS_KEY_PACKAGES_KEY);
            packages.insert(
                &mut txn,
                peer,
                hex::encode(identity.generate_key_package().unwrap()),
            );
        }
        retry_pending_approvals(&state).await;
        assert!(!is_pending(&state, peer));
        assert!(!state.approval_errors.contains_key(peer));
        assert_eq!(state.mls.lock().await.current_epoch(), Some(1));
        // A stale request after admission must not consume its grant again or
        // issue another add commit, even if the member list is incomplete.
        seed_pending(&state, peer);
        retry_pending_approvals(&state).await;
        assert!(!is_pending(&state, peer));
        assert_eq!(state.mls.lock().await.current_epoch(), Some(1));
    }

    /// The review case: a genuine rejoin whose KeyPackage time ties with its
    /// admission. The sweep must not take that for admission and drop the
    /// request, or the device waits forever for a Welcome.
    #[tokio::test]
    async fn automatic_sweep_keeps_a_rejoin_it_cannot_prove_and_admits_the_next_key() {
        let (state, _dir) = automatic_state().await;
        let peer_key = libp2p_identity::Keypair::generate_ed25519();
        let peer = peer_key.public().to_peer_id().to_string();
        // One fixed time for the tie, so it holds across a second boundary.
        let tie = crate::mls::identity::unix_now().unwrap() - 3600;
        let publish = |identity: &crate::mls::MlsIdentity, not_before: u64| {
            let kp = identity
                .generate_key_package_valid_from(not_before)
                .unwrap();
            let binding = crate::identity::sign_key_package_binding(
                &peer_key,
                &state.circle_id,
                identity.signer.public(),
            )
            .unwrap();
            let mut txn = state.control.transact_mut();
            let packages = txn.get_or_insert_map(MLS_KEY_PACKAGES_KEY);
            packages.insert(&mut txn, peer.as_str(), hex::encode(kp));
            let bindings = txn.get_or_insert_map(MLS_KEY_PACKAGE_BINDINGS_KEY);
            bindings.insert(&mut txn, peer.as_str(), binding);
        };

        publish(&crate::mls::MlsIdentity::generate(&peer).unwrap(), tie);
        seed_valid_request(&state, &peer);
        retry_pending_approvals(&state).await;
        assert!(!is_pending(&state, &peer));
        let admitted = state.mls.lock().await.current_epoch();

        let rejoined = crate::mls::MlsIdentity::generate(&peer).unwrap();
        publish(&rejoined, tie);
        seed_valid_request(&state, &peer);
        retry_pending_approvals(&state).await;
        assert!(is_pending(&state, &peer), "the request is kept");
        assert!(state
            .approval_errors
            .get(&peer)
            .unwrap()
            .contains("restart it"));
        assert_eq!(state.mls.lock().await.current_epoch(), admitted);

        // The device restarts and publishes a later KeyPackage.
        publish(&rejoined, tie + 3600);
        retry_pending_approvals(&state).await;
        assert!(!is_pending(&state, &peer));
        assert!(state.mls.lock().await.current_epoch() > admitted);
    }

    #[tokio::test]
    async fn automatic_sweep_keeps_mls_failure_pending_despite_provisional_member() {
        let (state, _dir) = automatic_state().await;
        let peer = "peer-joiner";
        seed_valid_request(&state, peer);
        {
            let mut txn = state.control.transact_mut();
            let members = txn.get_or_insert_map(MEMBER_LIST_KEY);
            members.insert(&mut txn, peer, "{}");
            let packages = txn.get_or_insert_map(MLS_KEY_PACKAGES_KEY);
            packages.insert(&mut txn, peer, hex::encode(b"invalid MLS key package"));
        }
        retry_pending_approvals(&state).await;
        assert!(is_pending(&state, peer));
        assert!(state
            .approval_errors
            .get(peer)
            .unwrap()
            .contains("MLS admission failed"));
        assert_eq!(state.mls.lock().await.current_epoch(), Some(0));
    }

    #[tokio::test]
    async fn automatic_sweep_does_not_admit_without_valid_invite() {
        let (state, _dir) = automatic_state().await;
        let peer = "peer-joiner";
        seed_pending(&state, peer);
        {
            let identity = crate::mls::MlsIdentity::generate(peer).unwrap();
            let mut txn = state.control.transact_mut();
            let packages = txn.get_or_insert_map(MLS_KEY_PACKAGES_KEY);
            packages.insert(
                &mut txn,
                peer,
                hex::encode(identity.generate_key_package().unwrap()),
            );
        }
        retry_pending_approvals(&state).await;
        assert!(is_pending(&state, peer));
        assert_eq!(
            state.approval_errors.get(peer).unwrap().value(),
            "no invite grant presented"
        );
        assert_eq!(state.mls.lock().await.current_epoch(), Some(0));
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
