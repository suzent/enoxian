use axum::{extract::{Path, State}, http::StatusCode, response::IntoResponse, Json};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use yrs::{Any, Map, MapRef, Out, Transact};

use crate::control::{CircleEvent, MemberEntry, MemberRole, MEMBER_LIST_KEY};
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

    // Verify admin signature of "add:{peer_id}:{role}"
    let msg = format!("add:{}:{}", req.peer_id, role);
    if let Err(e) = verify_admin_sig(&state.admin_pubkey_hex, msg.as_bytes(), &req.admin_signature) {
        return (StatusCode::FORBIDDEN, Json(json!({"error": format!("invalid admin signature: {e}")}))).into_response();
    }

    let entry = MemberEntry {
        peer_id: req.peer_id.clone(),
        agent_id: req.agent_id.unwrap_or_default(),
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

    let msg = format!("add:{}:admin", req.peer_id);
    if let Err(e) = verify_admin_sig(&state.admin_pubkey_hex, msg.as_bytes(), &req.admin_signature) {
        return (StatusCode::FORBIDDEN, Json(json!({"error": format!("invalid admin signature: {e}")}))).into_response();
    }

    // Update existing entry role to admin, or insert new admin entry
    let map: MapRef = state.control.get_or_insert_map(MEMBER_LIST_KEY);
    let existing = {
        let txn = state.control.transact();
        map.get(&txn, req.peer_id.as_str()).and_then(|v| {
            if let Out::Any(Any::String(s)) = v {
                serde_json::from_str::<MemberEntry>(&s).ok()
            } else {
                None
            }
        })
    };

    let entry = MemberEntry {
        peer_id: req.peer_id.clone(),
        agent_id: existing.map(|e| e.agent_id).unwrap_or_default(),
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
