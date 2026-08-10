use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use yrs::{Any, Array, Map, MapRef, Out, Transact};

use crate::control::{
    CircleEvent, MemberEntry, MemberRole, MlsCommitEntry, PendingEntry, MEMBER_LIST_KEY,
    MLS_COMMITS_KEY, MLS_KEY_PACKAGES_KEY, MLS_PENDING_KEY, MLS_REMOVED_KEY, MLS_WELCOMES_KEY,
};
use crate::daemon::DaemonState;

pub async fn list_members(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "circle not found"})),
            )
                .into_response()
        }
    };
    let map: MapRef = state.control.get_or_insert_map(MEMBER_LIST_KEY);
    let txn = state.control.transact();
    let mut members: Vec<MemberEntry> = Vec::new();
    for (_, val) in map.iter(&txn) {
        if let Out::Any(Any::String(s)) = val {
            if let Ok(m) = serde_json::from_str::<MemberEntry>(&s) {
                members.push(m);
            }
        }
    }
    members.sort_by_key(|a| a.added_at);
    Json(members).into_response()
}

#[derive(Deserialize)]
pub struct AddMemberRequest {
    pub peer_id: String,
    /// Human owner of this peer ("alice"). Defaults to agent_id if omitted.
    pub owner: Option<String>,
    pub agent_id: Option<String>,
    pub device_label: Option<String>,
    pub agents: Option<Vec<String>>,
    pub role: Option<String>,
    pub admin_signature: String,
}

pub async fn add_member(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Json(req): Json<AddMemberRequest>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "circle not found"})),
            )
                .into_response()
        }
    };

    let role = match req.role.as_deref().unwrap_or("member") {
        "admin" => MemberRole::Admin,
        _ => MemberRole::Member,
    };

    let agent_id = req.agent_id.unwrap_or_default();
    let owner = req.owner.unwrap_or_else(|| agent_id.clone());

    // Verify admin signature of "add:{peer_id}:{role}:owner:{owner}"
    // If frontend omits the signature, auto-sign with the local admin.key.
    let msg = format!("add:{}:{}:owner:{}", req.peer_id, role, owner);
    let sig = match resolve_admin_sig(&circle_id, msg.as_bytes(), &req.admin_signature) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({"error": format!("admin signature required: {e}")})),
            )
                .into_response()
        }
    };
    if let Err(e) = verify_admin_sig(&state.admin_pubkey_hex, msg.as_bytes(), &sig) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": format!("invalid admin signature: {e}")})),
        )
            .into_response();
    }

    // Note: owner names are per-person, not per-device. The same person can have
    // multiple devices with the same owner name; peer_id is the unique identifier.

    let entry = MemberEntry {
        peer_id: req.peer_id.clone(),
        owner,
        agent_id,
        device_label: req.device_label.unwrap_or_default(),
        agents: req.agents.unwrap_or_default(),
        role,
        added_at: Utc::now(),
        signature: sig,
    };
    let Ok(json_str) = serde_json::to_string(&entry) else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "serialize failed"})),
        )
            .into_response();
    };

    {
        let map: MapRef = state.control.get_or_insert_map(MEMBER_LIST_KEY);
        let mut txn = state.control.transact_mut();
        map.insert(&mut txn, req.peer_id.as_str(), json_str.as_str());
    }

    let _ = state.events.send(CircleEvent::MemberAdded {
        peer_id: req.peer_id.clone(),
    });
    Json(json!({"status": "added", "peer_id": req.peer_id})).into_response()
}

#[derive(Deserialize)]
pub struct RemoveMemberRequest {
    pub peer_id: String,
    pub admin_signature: String,
}

pub async fn remove_member(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Json(req): Json<RemoveMemberRequest>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "circle not found"})),
            )
                .into_response()
        }
    };

    let msg = format!("remove:{}", req.peer_id);
    let sig = match resolve_admin_sig(&circle_id, msg.as_bytes(), &req.admin_signature) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({"error": format!("admin signature required: {e}")})),
            )
                .into_response()
        }
    };
    if let Err(e) = verify_admin_sig(&state.admin_pubkey_hex, msg.as_bytes(), &sig) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": format!("invalid admin signature: {e}")})),
        )
            .into_response();
    }

    // Issue MLS Remove commit so all remaining members advance their epoch,
    // deriving a new PSK that the evicted peer cannot compute.
    //
    // Phase 1: all MLS operations under the lock → produce owned results.
    // Phase 2: CRDT writes and disk save outside the lock.
    struct MlsRemoveOut {
        commit_bytes: Vec<u8>,
        epoch: u64,
    }
    let mls_out: Option<MlsRemoveOut> = {
        let mut mls_locked = state.mls.blocking_lock();
        // Safety: identity lives inside the MutexGuard; both are only accessed
        // within this block, so the raw-pointer cast is safe.
        let identity_ptr = &mls_locked.identity as *const _;
        let identity = unsafe { &*identity_ptr };
        match mls_locked.group.as_mut() {
            None => None, // no MLS group — CRDT-only removal
            Some(group) => {
                match group.leaf_index_for_peer(&req.peer_id) {
                    None => None, // peer never joined MLS; evict from CRDT only
                    Some(leaf_idx) => {
                        let commit_bytes =
                            match group.remove_member(identity, leaf_idx) {
                                Ok(b) => b,
                                Err(e) => return (
                                    StatusCode::INTERNAL_SERVER_ERROR,
                                    Json(
                                        json!({"error": format!("MLS remove_member failed: {e}")}),
                                    ),
                                )
                                    .into_response(),
                            };
                        let epoch = group.epoch();
                        Some(MlsRemoveOut {
                            commit_bytes,
                            epoch,
                        })
                    }
                }
            }
        }
        // mls_locked drops here
    };

    // Phase 2: broadcast commit + save group (outside the MLS lock).
    // The MLS epoch advances for remaining members, but we no longer derive or
    // rotate a transport PSK from it — the transport key is a stable per-circle
    // gate and eviction is the mls_removed tombstone written below. See
    // docs/plan/identity.md.
    if let Some(out) = mls_out {
        // Broadcast the Remove commit to remaining members via the CRDT.
        let entry = MlsCommitEntry {
            epoch: out.epoch,
            data_hex: hex::encode(&out.commit_bytes),
            sender_peer_id: state.peer_id.clone(),
            ratchet_tree_hex: String::new(), // not needed for Remove commits
        };
        if let Ok(json_str) = serde_json::to_string(&entry) {
            let commits_arr = state.control.get_or_insert_array(MLS_COMMITS_KEY);
            let mut txn = state.control.transact_mut();
            commits_arr.push_back(&mut txn, json_str.as_str());
        }

        // Persist the updated MLS group to disk.
        let mls_locked = state.mls.blocking_lock();
        if let Some(group) = &mls_locked.group {
            let _ = group.save(&mls_locked.identity, &state.circle_dir);
        }
    }

    // Read the agent_id before removing the member entry (needed for presence update).
    let evicted_agent_id: Option<String> = {
        let member_map: MapRef = state.control.get_or_insert_map(MEMBER_LIST_KEY);
        let txn = state.control.transact();
        member_map.get(&txn, req.peer_id.as_str()).and_then(|v| {
            if let Out::Any(Any::String(s)) = v {
                serde_json::from_str::<MemberEntry>(&s)
                    .ok()
                    .map(|e| e.agent_id)
            } else {
                None
            }
        })
    };

    // Remove from CRDT member list, clean up auxiliary keys, and write tombstone.
    // The tombstone (mls_removed map) is the sync-level gate: sync.rs rejects any
    // peer found here before exchanging CRDT data, even during the brief window
    // before PSK rotation completes.  All changes go in one transaction so peers
    // receiving the CRDT update see a consistent state.
    {
        let member_map: MapRef = state.control.get_or_insert_map(MEMBER_LIST_KEY);
        let kp_map: MapRef = state.control.get_or_insert_map(MLS_KEY_PACKAGES_KEY);
        let welcome_map: MapRef = state.control.get_or_insert_map(MLS_WELCOMES_KEY);
        let pending_map: MapRef = state.control.get_or_insert_map(MLS_PENDING_KEY);
        let removed_map: MapRef = state.control.get_or_insert_map(MLS_REMOVED_KEY);
        let removed_at = Utc::now().to_rfc3339();
        let mut txn = state.control.transact_mut();
        member_map.remove(&mut txn, req.peer_id.as_str());
        kp_map.remove(&mut txn, req.peer_id.as_str());
        welcome_map.remove(&mut txn, req.peer_id.as_str());
        pending_map.remove(&mut txn, req.peer_id.as_str());
        removed_map.insert(&mut txn, req.peer_id.as_str(), removed_at.as_str());
    }

    // Mark the evicted peer as offline in presence.
    if let Some(agent_id) = &evicted_agent_id {
        crate::presence::write_offline(&state, agent_id);
    }

    let _ = state.events.send(CircleEvent::MemberRemoved {
        peer_id: req.peer_id.clone(),
    });

    // No transport-PSK rotation: the mls_removed tombstone written above is the
    // eviction boundary (sync.rs rejects tombstoned peers before any data). The
    // transport PSK stays stable so legitimate members never get locked out by
    // an epoch they missed. See docs/plan/identity.md.

    Json(json!({"status": "removed", "peer_id": req.peer_id})).into_response()
}

#[derive(Deserialize)]
pub struct PromoteMemberRequest {
    pub peer_id: String,
    pub admin_signature: String,
}

pub async fn promote_member(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Json(req): Json<PromoteMemberRequest>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "circle not found"})),
            )
                .into_response()
        }
    };

    let existing_owner = {
        let map: MapRef = state.control.get_or_insert_map(MEMBER_LIST_KEY);
        let txn = state.control.transact();
        map.get(&txn, req.peer_id.as_str())
            .and_then(|v| {
                if let Out::Any(Any::String(s)) = v {
                    serde_json::from_str::<MemberEntry>(&s)
                        .ok()
                        .map(|e| e.owner)
                } else {
                    None
                }
            })
            .unwrap_or_default()
    };
    let msg = format!("add:{}:admin:owner:{}", req.peer_id, existing_owner);
    let sig = match resolve_admin_sig(&circle_id, msg.as_bytes(), &req.admin_signature) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({"error": format!("admin signature required: {e}")})),
            )
                .into_response()
        }
    };
    if let Err(e) = verify_admin_sig(&state.admin_pubkey_hex, msg.as_bytes(), &sig) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": format!("invalid admin signature: {e}")})),
        )
            .into_response();
    }

    let map: MapRef = state.control.get_or_insert_map(MEMBER_LIST_KEY);
    let (prev_owner, prev_agent_id, prev_device_label, prev_agents) = {
        let txn = state.control.transact();
        map.get(&txn, req.peer_id.as_str())
            .and_then(|v| {
                if let Out::Any(Any::String(s)) = v {
                    serde_json::from_str::<MemberEntry>(&s)
                        .ok()
                        .map(|e| (e.owner, e.agent_id, e.device_label, e.agents))
                } else {
                    None
                }
            })
            .unwrap_or_default()
    };
    let entry = MemberEntry {
        peer_id: req.peer_id.clone(),
        owner: prev_owner,
        agent_id: prev_agent_id,
        device_label: prev_device_label,
        agents: prev_agents,
        role: MemberRole::Admin,
        added_at: Utc::now(),
        signature: sig,
    };
    let Ok(json_str) = serde_json::to_string(&entry) else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "serialize failed"})),
        )
            .into_response();
    };

    {
        let mut txn = state.control.transact_mut();
        map.insert(&mut txn, req.peer_id.as_str(), json_str.as_str());
    }

    Json(json!({"status": "promoted", "peer_id": req.peer_id})).into_response()
}

pub async fn list_pending(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "circle not found"})),
            )
                .into_response()
        }
    };
    let map: MapRef = state.control.get_or_insert_map(MLS_PENDING_KEY);
    let txn = state.control.transact();
    let mut entries: Vec<PendingEntry> = Vec::new();
    for (_, val) in map.iter(&txn) {
        if let Out::Any(Any::String(s)) = val {
            if let Ok(e) = serde_json::from_str::<PendingEntry>(&s) {
                entries.push(e);
            }
        }
    }
    entries.sort_by_key(|a| a.requested_at);
    Json(entries).into_response()
}

#[derive(Deserialize)]
pub struct ApproveMemberRequest {
    pub peer_id: String,
    pub role: Option<String>,
    pub owner: String,
    pub admin_signature: String,
    /// Override the agents list from the pending entry (optional).
    pub agents: Option<Vec<String>>,
}

pub async fn approve_member(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Json(req): Json<ApproveMemberRequest>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "circle not found"})),
            )
                .into_response()
        }
    };

    let role_str = req.role.as_deref().unwrap_or("member");
    let role = match role_str {
        "admin" => MemberRole::Admin,
        _ => MemberRole::Member,
    };

    let msg = format!("add:{}:{}:owner:{}", req.peer_id, role, req.owner);
    let sig = match resolve_admin_sig(&circle_id, msg.as_bytes(), &req.admin_signature) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({"error": format!("admin signature required: {e}")})),
            )
                .into_response()
        }
    };
    if let Err(e) = verify_admin_sig(&state.admin_pubkey_hex, msg.as_bytes(), &sig) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": format!("invalid admin signature: {e}")})),
        )
            .into_response();
    }

    // Load key package
    let kp_hex = {
        let kp_map: MapRef = state.control.get_or_insert_map(MLS_KEY_PACKAGES_KEY);
        let txn = state.control.transact();
        match kp_map.get(&txn, req.peer_id.as_str()) {
            Some(Out::Any(Any::String(s))) => s.to_string(),
            _ => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": "key package not found"})),
                )
                    .into_response()
            }
        }
    };
    let kp_bytes = match hex::decode(&kp_hex) {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("invalid key package hex: {e}")})),
            )
                .into_response()
        }
    };

    // Add MLS member
    let (commit_bytes, welcome_bytes, ratchet_tree_bytes) = {
        let mut mls_locked = state.mls.blocking_lock();
        let identity_ptr = &mls_locked.identity as *const _;
        let group = match mls_locked.group.as_mut() {
            Some(g) => g,
            None => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": "MLS group not initialized"})),
                )
                    .into_response()
            }
        };
        let identity = unsafe { &*identity_ptr };
        match group.add_member(identity, &kp_bytes) {
            Ok(t) => t,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": format!("MLS add_member failed: {e}")})),
                )
                    .into_response()
            }
        }
    };

    let welcome_hex = hex::encode(&welcome_bytes);
    let commit_hex = hex::encode(&commit_bytes);
    let ratchet_hex = hex::encode(&ratchet_tree_bytes);

    {
        let welcomes_map: MapRef = state.control.get_or_insert_map(MLS_WELCOMES_KEY);
        let mut txn = state.control.transact_mut();
        welcomes_map.insert(&mut txn, req.peer_id.as_str(), welcome_hex.as_str());
    }

    {
        let commits_arr = state.control.get_or_insert_array(MLS_COMMITS_KEY);
        let epoch = {
            let mls_locked = state.mls.blocking_lock();
            mls_locked.group.as_ref().map(|g| g.epoch()).unwrap_or(0)
        };
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

    // Add to member list, remove from pending — carry device_label and agents from pending entry
    let (agent_id, device_label, agents) = {
        let pending_map: MapRef = state.control.get_or_insert_map(MLS_PENDING_KEY);
        let txn = state.control.transact();
        pending_map
            .get(&txn, req.peer_id.as_str())
            .and_then(|v| {
                if let Out::Any(Any::String(s)) = v {
                    serde_json::from_str::<PendingEntry>(&s)
                        .ok()
                        .map(|e| (e.agent_id, e.device_label, e.agents))
                } else {
                    None
                }
            })
            .unwrap_or_default()
    };

    let entry = MemberEntry {
        peer_id: req.peer_id.clone(),
        owner: req.owner.clone(),
        agent_id,
        device_label,
        agents: req.agents.clone().unwrap_or(agents),
        role,
        added_at: Utc::now(),
        signature: sig,
    };
    let Ok(json_str) = serde_json::to_string(&entry) else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "serialize failed"})),
        )
            .into_response();
    };

    {
        let member_map: MapRef = state.control.get_or_insert_map(MEMBER_LIST_KEY);
        let pending_map: MapRef = state.control.get_or_insert_map(MLS_PENDING_KEY);
        let mut txn = state.control.transact_mut();
        member_map.insert(&mut txn, req.peer_id.as_str(), json_str.as_str());
        pending_map.remove(&mut txn, req.peer_id.as_str());
    }

    // Save MLS group
    {
        let mls_locked = state.mls.blocking_lock();
        if let Some(group) = &mls_locked.group {
            let _ = group.save(&mls_locked.identity, &state.circle_dir);
        }
    }

    let _ = state.events.send(CircleEvent::MemberAdded {
        peer_id: req.peer_id.clone(),
    });
    Json(json!({"status": "approved", "peer_id": req.peer_id})).into_response()
}

#[derive(Deserialize)]
pub struct RejectMemberRequest {
    pub peer_id: String,
    pub admin_signature: String,
}

pub async fn reject_member(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Json(req): Json<RejectMemberRequest>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "circle not found"})),
            )
                .into_response()
        }
    };

    let msg = format!("reject:{}", req.peer_id);
    let sig = match resolve_admin_sig(&circle_id, msg.as_bytes(), &req.admin_signature) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({"error": format!("admin signature required: {e}")})),
            )
                .into_response()
        }
    };
    if let Err(e) = verify_admin_sig(&state.admin_pubkey_hex, msg.as_bytes(), &sig) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": format!("invalid admin signature: {e}")})),
        )
            .into_response();
    }

    {
        let pending_map: MapRef = state.control.get_or_insert_map(MLS_PENDING_KEY);
        let mut txn = state.control.transact_mut();
        pending_map.remove(&mut txn, req.peer_id.as_str());
    }

    Json(json!({"status": "rejected", "peer_id": req.peer_id})).into_response()
}

fn verify_admin_sig(admin_pubkey_hex: &str, msg: &[u8], sig_hex: &str) -> anyhow::Result<()> {
    if admin_pubkey_hex.is_empty() {
        anyhow::bail!("no admin pubkey configured for this circle");
    }
    let pubkey_bytes = hex::decode(admin_pubkey_hex)?;
    let pubkey = libp2p::identity::PublicKey::try_decode_protobuf(&pubkey_bytes)
        .map_err(|e| anyhow::anyhow!("invalid admin pubkey: {e}"))?;
    let sig = hex::decode(sig_hex)?;
    if !pubkey.verify(msg, &sig) {
        anyhow::bail!("signature mismatch");
    }
    Ok(())
}

/// When the frontend omits admin_signature (empty string), try to sign using the
/// daemon's local admin.key for this circle.  Returns the hex signature on success,
/// or an error if admin.key is not present (i.e. this machine is not the admin).
fn local_admin_sign(circle_id: &str, msg: &[u8]) -> anyhow::Result<String> {
    use crate::{config::circle_dir, crypto::keypair_from_hex};
    let key_path = circle_dir(circle_id)?.join("admin.key");
    let hex_str = std::fs::read_to_string(&key_path)
        .map_err(|_| anyhow::anyhow!("not admin: admin.key not found for this circle"))?;
    let kp = keypair_from_hex(hex_str.trim())?;
    let sig = kp
        .sign(msg)
        .map_err(|e| anyhow::anyhow!("signing failed: {e}"))?;
    Ok(hex::encode(sig))
}

/// Resolve the admin signature: use the provided one if non-empty, otherwise
/// auto-sign with the local admin.key.  Returns Err if neither is available.
fn resolve_admin_sig(circle_id: &str, msg: &[u8], provided: &str) -> anyhow::Result<String> {
    if !provided.is_empty() {
        return Ok(provided.to_string());
    }
    local_admin_sign(circle_id, msg)
}
