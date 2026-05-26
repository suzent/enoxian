use axum::{extract::{Path, State}, http::StatusCode, response::IntoResponse, Json};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use yrs::{Any, Array, Map, MapRef, Out, Transact};

use crate::control::{CircleEvent, MemberEntry, MemberRole, MlsCommitEntry, PendingEntry, MEMBER_LIST_KEY, MLS_KEY_PACKAGES_KEY, MLS_PENDING_KEY, MLS_WELCOMES_KEY, MLS_COMMITS_KEY};
use crate::daemon::DaemonState;

pub async fn list_members(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => return (StatusCode::NOT_FOUND, Json(json!({"error": "circle not found"}))).into_response(),
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
    members.sort_by(|a, b| a.added_at.cmp(&b.added_at));
    Json(members).into_response()
}

#[derive(Deserialize)]
pub struct AddMemberRequest {
    pub peer_id: String,
    /// Human owner of this peer ("alice"). Defaults to agent_id if omitted.
    pub owner: Option<String>,
    pub agent_id: Option<String>,
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
        None => return (StatusCode::NOT_FOUND, Json(json!({"error": "circle not found"}))).into_response(),
    };

    let role = match req.role.as_deref().unwrap_or("member") {
        "admin" => MemberRole::Admin,
        _ => MemberRole::Member,
    };

    let agent_id = req.agent_id.unwrap_or_default();
    let owner = req.owner.unwrap_or_else(|| agent_id.clone());

    // Verify admin signature of "add:{peer_id}:{role}:owner:{owner}"
    let msg = format!("add:{}:{}:owner:{}", req.peer_id, role, owner);
    if let Err(e) = verify_admin_sig(&state.admin_pubkey_hex, msg.as_bytes(), &req.admin_signature) {
        return (StatusCode::FORBIDDEN, Json(json!({"error": format!("invalid admin signature: {e}")}))).into_response();
    }

    // Owner uniqueness check
    {
        let map: MapRef = state.control.get_or_insert_map(MEMBER_LIST_KEY);
        let txn = state.control.transact();
        for (key, val) in map.iter(&txn) {
            if key == req.peer_id.as_str() { continue; }
            if let Out::Any(Any::String(s)) = val {
                if let Ok(m) = serde_json::from_str::<MemberEntry>(&s) {
                    if m.owner == owner {
                        return (StatusCode::CONFLICT, Json(json!({"error": format!("owner '{}' already registered", owner)}))).into_response();
                    }
                }
            }
        }
    }

    let entry = MemberEntry {
        peer_id: req.peer_id.clone(),
        owner,
        agent_id,
        role,
        added_at: Utc::now(),
        signature: req.admin_signature,
    };
    let Ok(json_str) = serde_json::to_string(&entry) else {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "serialize failed"}))).into_response();
    };

    {
        let map: MapRef = state.control.get_or_insert_map(MEMBER_LIST_KEY);
        let mut txn = state.control.transact_mut();
        map.insert(&mut txn, req.peer_id.as_str(), json_str.as_str());
    }

    let _ = state.events.send(CircleEvent::MemberAdded { peer_id: req.peer_id.clone() });
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
        None => return (StatusCode::NOT_FOUND, Json(json!({"error": "circle not found"}))).into_response(),
    };

    let msg = format!("remove:{}", req.peer_id);
    if let Err(e) = verify_admin_sig(&state.admin_pubkey_hex, msg.as_bytes(), &req.admin_signature) {
        return (StatusCode::FORBIDDEN, Json(json!({"error": format!("invalid admin signature: {e}")}))).into_response();
    }

    {
        let map: MapRef = state.control.get_or_insert_map(MEMBER_LIST_KEY);
        let mut txn = state.control.transact_mut();
        map.remove(&mut txn, req.peer_id.as_str());
    }

    let _ = state.events.send(CircleEvent::MemberRemoved { peer_id: req.peer_id.clone() });
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
        None => return (StatusCode::NOT_FOUND, Json(json!({"error": "circle not found"}))).into_response(),
    };

    let existing_owner = {
        let map: MapRef = state.control.get_or_insert_map(MEMBER_LIST_KEY);
        let txn = state.control.transact();
        map.get(&txn, req.peer_id.as_str()).and_then(|v| {
            if let Out::Any(Any::String(s)) = v {
                serde_json::from_str::<MemberEntry>(&s).ok().map(|e| e.owner)
            } else {
                None
            }
        }).unwrap_or_default()
    };
    let msg = format!("add:{}:admin:owner:{}", req.peer_id, existing_owner);
    if let Err(e) = verify_admin_sig(&state.admin_pubkey_hex, msg.as_bytes(), &req.admin_signature) {
        return (StatusCode::FORBIDDEN, Json(json!({"error": format!("invalid admin signature: {e}")}))).into_response();
    }

    let map: MapRef = state.control.get_or_insert_map(MEMBER_LIST_KEY);
    let (prev_owner, prev_agent_id) = {
        let txn = state.control.transact();
        map.get(&txn, req.peer_id.as_str()).and_then(|v| {
            if let Out::Any(Any::String(s)) = v {
                serde_json::from_str::<MemberEntry>(&s).ok().map(|e| (e.owner, e.agent_id))
            } else {
                None
            }
        }).unwrap_or_default()
    };
    let entry = MemberEntry {
        peer_id: req.peer_id.clone(),
        owner: prev_owner,
        agent_id: prev_agent_id,
        role: MemberRole::Admin,
        added_at: Utc::now(),
        signature: req.admin_signature,
    };
    let Ok(json_str) = serde_json::to_string(&entry) else {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "serialize failed"}))).into_response();
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
        None => return (StatusCode::NOT_FOUND, Json(json!({"error": "circle not found"}))).into_response(),
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
    entries.sort_by(|a, b| a.requested_at.cmp(&b.requested_at));
    Json(entries).into_response()
}

#[derive(Deserialize)]
pub struct ApproveMemberRequest {
    pub peer_id: String,
    pub role: Option<String>,
    pub owner: String,
    pub admin_signature: String,
}

pub async fn approve_member(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Json(req): Json<ApproveMemberRequest>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => return (StatusCode::NOT_FOUND, Json(json!({"error": "circle not found"}))).into_response(),
    };

    let role_str = req.role.as_deref().unwrap_or("member");
    let role = match role_str {
        "admin" => MemberRole::Admin,
        _ => MemberRole::Member,
    };

    let msg = format!("add:{}:{}:owner:{}", req.peer_id, role, req.owner);
    if let Err(e) = verify_admin_sig(&state.admin_pubkey_hex, msg.as_bytes(), &req.admin_signature) {
        return (StatusCode::FORBIDDEN, Json(json!({"error": format!("invalid admin signature: {e}")}))).into_response();
    }

    // Owner uniqueness check
    {
        let map: MapRef = state.control.get_or_insert_map(MEMBER_LIST_KEY);
        let txn = state.control.transact();
        for (key, val) in map.iter(&txn) {
            if key == req.peer_id.as_str() { continue; }
            if let Out::Any(Any::String(s)) = val {
                if let Ok(m) = serde_json::from_str::<MemberEntry>(&s) {
                    if m.owner == req.owner {
                        return (StatusCode::CONFLICT, Json(json!({"error": format!("owner '{}' already registered", req.owner)}))).into_response();
                    }
                }
            }
        }
    }

    // Load key package
    let kp_hex = {
        let kp_map: MapRef = state.control.get_or_insert_map(MLS_KEY_PACKAGES_KEY);
        let txn = state.control.transact();
        match kp_map.get(&txn, req.peer_id.as_str()) {
            Some(Out::Any(Any::String(s))) => s.to_string(),
            _ => return (StatusCode::BAD_REQUEST, Json(json!({"error": "key package not found"}))).into_response(),
        }
    };
    let kp_bytes = match hex::decode(&kp_hex) {
        Ok(b) => b,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": format!("invalid key package hex: {e}")}))).into_response(),
    };

    // Add MLS member
    let (commit_bytes, welcome_bytes, ratchet_tree_bytes) = {
        let mut mls_locked = state.mls.blocking_lock();
        let identity_ptr = &mls_locked.identity as *const _;
        let group = match mls_locked.group.as_mut() {
            Some(g) => g,
            None => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "MLS group not initialized"}))).into_response(),
        };
        let identity = unsafe { &*identity_ptr };
        match group.add_member(identity, &kp_bytes) {
            Ok(t) => t,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("MLS add_member failed: {e}")}))).into_response(),
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

    // Add to member list, remove from pending
    let agent_id = {
        let pending_map: MapRef = state.control.get_or_insert_map(MLS_PENDING_KEY);
        let txn = state.control.transact();
        pending_map.get(&txn, req.peer_id.as_str()).and_then(|v| {
            if let Out::Any(Any::String(s)) = v {
                serde_json::from_str::<PendingEntry>(&s).ok().map(|e| e.agent_id)
            } else {
                None
            }
        }).unwrap_or_default()
    };

    let entry = MemberEntry {
        peer_id: req.peer_id.clone(),
        owner: req.owner.clone(),
        agent_id,
        role,
        added_at: Utc::now(),
        signature: req.admin_signature,
    };
    let Ok(json_str) = serde_json::to_string(&entry) else {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "serialize failed"}))).into_response();
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

    let _ = state.events.send(CircleEvent::MemberAdded { peer_id: req.peer_id.clone() });
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
        None => return (StatusCode::NOT_FOUND, Json(json!({"error": "circle not found"}))).into_response(),
    };

    let msg = format!("reject:{}", req.peer_id);
    if let Err(e) = verify_admin_sig(&state.admin_pubkey_hex, msg.as_bytes(), &req.admin_signature) {
        return (StatusCode::FORBIDDEN, Json(json!({"error": format!("invalid admin signature: {e}")}))).into_response();
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
