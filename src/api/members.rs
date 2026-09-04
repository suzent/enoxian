use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use yrs::{Any, Array, Map, Out, ReadTxn, Transact, WriteTxn};

use crate::control::{
    CircleEvent, MemberEntry, MemberRole, MlsCommitEntry, PendingEntry, MEMBER_LIST_KEY,
    MLS_COMMITS_KEY, MLS_KEY_PACKAGES_KEY, MLS_PENDING_KEY, MLS_REMOVED_KEY, MLS_WELCOMES_KEY,
};
use crate::daemon::DaemonState;

/// Retry budget for taking the control document while approving a member.
/// Nothing irreversible happens until both locks are held, so retrying is free;
/// giving up returns a busy response having changed nothing.
const APPROVAL_RETRIES: u32 = 10;
const APPROVAL_BACKOFF: std::time::Duration = std::time::Duration::from_millis(50);

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
    let txn = match state.control.try_transact() {
        Ok(txn) => txn,
        Err(_) => return super::circle_busy(),
    };
    let Some(map) = txn.get_map(MEMBER_LIST_KEY) else {
        return Json(Vec::<MemberEntry>::new()).into_response();
    };
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
        let mut txn = match state.control.try_transact_mut() {
            Ok(txn) => txn,
            Err(_) => return super::circle_busy(),
        };
        let map = txn.get_or_insert_map(MEMBER_LIST_KEY);
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
    // One unit of work, for the same reason as `approve_member`: the MLS
    // remove advances the epoch irreversibly, and the commit it returns is the
    // only way remaining members can follow. Publishing that commit in a
    // separate transaction meant a busy control document left every remaining
    // member unable to decrypt, with the handler answering "try again".
    //
    // Take the MLS lock and the control-document write transaction together,
    // then do the whole eviction — MLS commit, member removal, auxiliary key
    // cleanup and tombstone — with no await in between.
    let mut attempt: u32 = 0;
    let evicted_agent_id: Option<String> = loop {
        {
            let mut mls_locked = state.mls.lock().await;
            if let Ok(mut txn) = state.control.try_transact_mut() {
                // A circle with no MLS group, or a peer that never joined it,
                // is a CRDT-only eviction — no commit to publish.
                let mls_out = match mls_locked.remove_member_by_peer(&req.peer_id) {
                    Ok(out) => out,
                    Err(e) => {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({"error": format!("MLS remove_member failed: {e}")})),
                        )
                            .into_response()
                    }
                };

                if let Some((commit_bytes, epoch)) = mls_out {
                    let entry = MlsCommitEntry {
                        epoch,
                        data_hex: hex::encode(&commit_bytes),
                        sender_peer_id: state.peer_id.clone(),
                        ratchet_tree_hex: String::new(), // not needed for Remove commits
                    };
                    let Ok(json_str) = serde_json::to_string(&entry) else {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({
                                "error": "the MLS group advanced but its commit could not be serialized"
                            })),
                        )
                            .into_response();
                    };
                    let commits_arr = txn.get_or_insert_array(MLS_COMMITS_KEY);
                    commits_arr.push_back(&mut txn, json_str.as_str());
                }

                // Read the agent_id before the entry goes, for the presence update.
                let evicted = txn
                    .get_map(MEMBER_LIST_KEY)
                    .and_then(|member_map| member_map.get(&txn, req.peer_id.as_str()))
                    .and_then(|v| match v {
                        Out::Any(Any::String(s)) => serde_json::from_str::<MemberEntry>(&s)
                            .ok()
                            .map(|e| e.agent_id),
                        _ => None,
                    });

                // The tombstone is the eviction boundary: sync.rs rejects a
                // tombstoned peer before exchanging any data. Everything lands
                // in one transaction so peers see a consistent state.
                let removed_at = Utc::now().to_rfc3339();
                let member_map = txn.get_or_insert_map(MEMBER_LIST_KEY);
                let kp_map = txn.get_or_insert_map(MLS_KEY_PACKAGES_KEY);
                let welcome_map = txn.get_or_insert_map(MLS_WELCOMES_KEY);
                let pending_map = txn.get_or_insert_map(MLS_PENDING_KEY);
                let removed_map = txn.get_or_insert_map(MLS_REMOVED_KEY);
                member_map.remove(&mut txn, req.peer_id.as_str());
                kp_map.remove(&mut txn, req.peer_id.as_str());
                welcome_map.remove(&mut txn, req.peer_id.as_str());
                pending_map.remove(&mut txn, req.peer_id.as_str());
                removed_map.insert(&mut txn, req.peer_id.as_str(), removed_at.as_str());
                drop(txn);

                if let Err(e) = mls_locked.save(&state.circle_dir) {
                    tracing::error!(
                        "[member] evicted {} but failed to persist the MLS group: {e}",
                        req.peer_id
                    );
                }
                break evicted;
            }
        }
        attempt += 1;
        if attempt >= APPROVAL_RETRIES {
            return super::circle_busy();
        }
        tokio::time::sleep(APPROVAL_BACKOFF * attempt).await;
    };

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
    // an epoch they missed. See docs/concepts/security.md.

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
        let txn = match state.control.try_transact() {
            Ok(txn) => txn,
            Err(_) => return super::circle_busy(),
        };
        txn.get_map(MEMBER_LIST_KEY)
            .and_then(|map| map.get(&txn, req.peer_id.as_str()))
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

    let (prev_owner, prev_agent_id, prev_device_label, prev_agents) = {
        let txn = match state.control.try_transact() {
            Ok(txn) => txn,
            Err(_) => return super::circle_busy(),
        };
        txn.get_map(MEMBER_LIST_KEY)
            .and_then(|map| map.get(&txn, req.peer_id.as_str()))
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
        let mut txn = match state.control.try_transact_mut() {
            Ok(txn) => txn,
            Err(_) => return super::circle_busy(),
        };
        let map = txn.get_or_insert_map(MEMBER_LIST_KEY);
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
    let txn = match state.control.try_transact() {
        Ok(txn) => txn,
        Err(_) => return super::circle_busy(),
    };
    let Some(map) = txn.get_map(MLS_PENDING_KEY) else {
        return Json(Vec::<PendingEntry>::new()).into_response();
    };
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

    // Everything below is one unit of work.
    //
    // `add_member` advances the MLS epoch and cannot be undone, and the commit
    // it produces is the only way other devices can follow. Publishing it in a
    // separate transaction meant a busy control document could strand every
    // peer on the old epoch — and because the handler answered "try again", the
    // retry advanced the epoch a second time.
    //
    // So: take the MLS lock and the control-document write transaction together
    // and only then touch the group. If the document is busy, release both and
    // retry — nothing has happened yet. Once both are held there is no await
    // point before the writes commit, so the epoch advance and the commit that
    // describes it land together or not at all.
    let mut attempt: u32 = 0;
    let outcome = loop {
        {
            let mut mls_locked = state.mls.lock().await;
            if let Ok(mut txn) = state.control.try_transact_mut() {
                // Read what we need from the same transaction we will write to.
                let Some(kp_hex) = txn
                    .get_map(MLS_KEY_PACKAGES_KEY)
                    .and_then(|kp_map| kp_map.get(&txn, req.peer_id.as_str()))
                    .and_then(|v| match v {
                        Out::Any(Any::String(s)) => Some(s.to_string()),
                        _ => None,
                    })
                else {
                    break Err((StatusCode::BAD_REQUEST, "key package not found".to_string()));
                };
                let kp_bytes = match hex::decode(&kp_hex) {
                    Ok(b) => b,
                    Err(e) => {
                        break Err((
                            StatusCode::BAD_REQUEST,
                            format!("invalid key package hex: {e}"),
                        ))
                    }
                };

                let (agent_id, device_label, agents) = txn
                    .get_map(MLS_PENDING_KEY)
                    .and_then(|pending_map| pending_map.get(&txn, req.peer_id.as_str()))
                    .and_then(|v| match v {
                        Out::Any(Any::String(s)) => serde_json::from_str::<PendingEntry>(&s)
                            .ok()
                            .map(|e| (e.agent_id, e.device_label, e.agents)),
                        _ => None,
                    })
                    .unwrap_or_default();

                // Irreversible from here.
                let (commit_bytes, welcome_bytes, ratchet_tree_bytes) =
                    match mls_locked.add_member(&kp_bytes) {
                        Ok(t) => t,
                        Err(e) => {
                            break Err((
                                StatusCode::INTERNAL_SERVER_ERROR,
                                format!("MLS add_member failed: {e}"),
                            ))
                        }
                    };
                let epoch = mls_locked.current_epoch().unwrap_or(0);

                let commit_entry = MlsCommitEntry {
                    epoch,
                    data_hex: hex::encode(&commit_bytes),
                    sender_peer_id: state.peer_id.clone(),
                    ratchet_tree_hex: hex::encode(&ratchet_tree_bytes),
                };
                let member_entry = MemberEntry {
                    peer_id: req.peer_id.clone(),
                    owner: req.owner.clone(),
                    agent_id,
                    device_label,
                    agents: req.agents.clone().unwrap_or(agents),
                    role,
                    added_at: Utc::now(),
                    signature: sig,
                };
                // Serialize before writing anything. A failure here used to skip
                // the commit silently and still report success — the same
                // permanent divergence, with no error at all.
                let (Ok(commit_json), Ok(member_json)) = (
                    serde_json::to_string(&commit_entry),
                    serde_json::to_string(&member_entry),
                ) else {
                    break Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "the MLS group advanced but its commit could not be serialized; \
                         the member may need approving again"
                            .to_string(),
                    ));
                };

                let welcomes_map = txn.get_or_insert_map(MLS_WELCOMES_KEY);
                welcomes_map.insert(
                    &mut txn,
                    req.peer_id.as_str(),
                    hex::encode(&welcome_bytes).as_str(),
                );
                let commits_arr = txn.get_or_insert_array(MLS_COMMITS_KEY);
                commits_arr.push_back(&mut txn, commit_json.as_str());
                let member_map = txn.get_or_insert_map(MEMBER_LIST_KEY);
                member_map.insert(&mut txn, req.peer_id.as_str(), member_json.as_str());
                let pending_map = txn.get_or_insert_map(MLS_PENDING_KEY);
                pending_map.remove(&mut txn, req.peer_id.as_str());
                drop(txn);

                // Persist only once the commit is safely in the document. Not
                // silent: without the group on disk it rolls back to the old
                // epoch on restart while peers already hold the commit.
                if let Err(e) = mls_locked.save(&state.circle_dir) {
                    tracing::error!(
                        "[member] approved {} but failed to persist the MLS group: {e}",
                        req.peer_id
                    );
                }
                break Ok(());
            }
        }
        attempt += 1;
        if attempt >= APPROVAL_RETRIES {
            return super::circle_busy();
        }
        tokio::time::sleep(APPROVAL_BACKOFF * attempt).await;
    };

    if let Err((status, message)) = outcome {
        return (status, Json(json!({ "error": message }))).into_response();
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
        let mut txn = match state.control.try_transact_mut() {
            Ok(txn) => txn,
            Err(_) => return super::circle_busy(),
        };
        let pending_map = txn.get_or_insert_map(MLS_PENDING_KEY);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{JoinGrant, MLS_KEY_PACKAGES_KEY};
    use crate::daemon::DaemonState;
    use crate::mls::{group::MlsGroupManager, MlsIdentity};
    use crate::state::AppState;
    use libp2p::identity::Keypair;
    use std::path::PathBuf;
    use tokio_util::sync::CancellationToken;
    use yrs::{Map, ReadTxn, Transact, WriteTxn};

    const CIRCLE: &str = "circle-under-test";

    struct Fixture {
        daemon: DaemonState,
        state: AppState,
        admin: Keypair,
        _dir: tempfile::TempDir,
    }

    /// A circle with a live MLS group, as the member handlers expect to find it.
    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let admin = Keypair::generate_ed25519();
        let identity = MlsIdentity::generate("admin").unwrap();
        let group = MlsGroupManager::create(&identity).unwrap();
        let mls = crate::mls::new_mls_state(identity, Some(group));

        let state = AppState::new(
            CIRCLE.into(),
            "test".into(),
            PathBuf::from(dir.path()),
            PathBuf::from(dir.path()),
            hex::encode(admin.public().encode_protobuf()),
            "admin-agent".into(),
            1,
            Keypair::generate_ed25519()
                .public()
                .to_peer_id()
                .to_string(),
            crate::config::JoinPolicy::Manual,
            "owner".into(),
            mls,
        );
        let daemon = DaemonState::new();
        daemon.insert_circle(CIRCLE.into(), state.clone(), CancellationToken::new());
        Fixture {
            daemon,
            state,
            admin,
            _dir: dir,
        }
    }

    fn sign(admin: &Keypair, msg: &str) -> String {
        hex::encode(admin.sign(msg.as_bytes()).unwrap())
    }

    /// Register a candidate the way a joining device does, so approve has a
    /// KeyPackage to consume.
    ///
    /// The MLS credential must carry the peer id, not the owner name:
    /// `leaf_index_for_peer` matches on it, so a credential built any other way
    /// leaves the member unremovable at the MLS layer. `enter.rs` does the same.
    fn candidate(fx: &Fixture, _owner: &str) -> String {
        let peer = Keypair::generate_ed25519()
            .public()
            .to_peer_id()
            .to_string();
        let identity = MlsIdentity::generate(&peer).unwrap();
        let kp = identity.generate_key_package().unwrap();
        let mut txn = fx.state.control.transact_mut();
        let packages = txn.get_or_insert_map(MLS_KEY_PACKAGES_KEY);
        packages.insert(&mut txn, peer.as_str(), hex::encode(kp).as_str());
        peer
    }

    fn map_has(fx: &Fixture, key: &str, field: &str) -> bool {
        let txn = fx.state.control.transact();
        txn.get_map(key).and_then(|m| m.get(&txn, field)).is_some()
    }

    fn commit_count(fx: &Fixture) -> u32 {
        let txn = fx.state.control.transact();
        txn.get_array(MLS_COMMITS_KEY)
            .map(|a| a.len(&txn))
            .unwrap_or(0)
    }

    fn epoch(fx: &Fixture) -> u64 {
        futures::executor::block_on(async {
            fx.state.mls.lock().await.current_epoch().unwrap_or(0)
        })
    }

    fn grant(fx: &Fixture) -> JoinGrant {
        // Issued by the admin, who is a member in good standing.
        let expires_at = Utc::now() + chrono::Duration::days(1);
        let g = crate::invite::sign_grant(
            CIRCLE,
            &crate::crypto::keypair_to_hex(&fx.admin).unwrap(),
            expires_at,
        )
        .unwrap();
        JoinGrant {
            inviter_pubkey_hex: g.inviter_pubkey_hex,
            nonce: g.nonce,
            sig: g.sig,
            expires_at,
        }
    }

    // ── Signature verification ────────────────────────────────────────────────

    #[test]
    fn admin_signature_verification_accepts_only_the_exact_message() {
        let fx = fixture();
        let pubkey = hex::encode(fx.admin.public().encode_protobuf());
        let msg = b"add:peer-1:member:owner:alice";

        assert!(verify_admin_sig(
            &pubkey,
            msg,
            &sign(&fx.admin, "add:peer-1:member:owner:alice")
        )
        .is_ok());
        // A different message, which is exactly how the CLI's format drifted
        // out of step with the daemon's and every add was rejected.
        assert!(verify_admin_sig(&pubkey, msg, &sign(&fx.admin, "add:peer-1:member")).is_err());
        // A different signer.
        let impostor = Keypair::generate_ed25519();
        assert!(verify_admin_sig(
            &pubkey,
            msg,
            &sign(&impostor, "add:peer-1:member:owner:alice")
        )
        .is_err());
        assert!(verify_admin_sig(&pubkey, msg, "not-hex").is_err());
        assert!(verify_admin_sig("", msg, &sign(&fx.admin, "x")).is_err());
    }

    // ── Approval ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn approve_publishes_the_commit_with_the_member_entry() {
        let fx = fixture();
        let peer = candidate(&fx, "alice");
        let before = epoch(&fx);

        let sig = sign(&fx.admin, &format!("add:{peer}:member:owner:alice"));
        let resp = approve_member(
            axum::extract::State(fx.daemon.clone()),
            axum::extract::Path(CIRCLE.into()),
            axum::Json(ApproveMemberRequest {
                peer_id: peer.clone(),
                role: None,
                owner: "alice".into(),
                admin_signature: sig,
                agents: None,
            }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::OK);

        // The epoch advanced, and the commit describing that advance is in the
        // shared log. If the two ever come apart, every other device is
        // stranded on the previous epoch and can decrypt nothing.
        assert!(epoch(&fx) > before, "approval must advance the MLS epoch");
        assert_eq!(commit_count(&fx), 1, "the commit must be published");
        assert!(map_has(&fx, MEMBER_LIST_KEY, &peer), "member recorded");
        assert!(map_has(&fx, MLS_WELCOMES_KEY, &peer), "welcome published");
    }

    #[tokio::test]
    async fn approve_with_a_bad_signature_changes_nothing() {
        let fx = fixture();
        let peer = candidate(&fx, "alice");
        let before = epoch(&fx);

        let resp = approve_member(
            axum::extract::State(fx.daemon.clone()),
            axum::extract::Path(CIRCLE.into()),
            axum::Json(ApproveMemberRequest {
                peer_id: peer.clone(),
                role: None,
                owner: "alice".into(),
                admin_signature: sign(&fx.admin, "add:someone-else:member:owner:alice"),
                agents: None,
            }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        // Nothing irreversible may happen before the signature is checked.
        assert_eq!(
            epoch(&fx),
            before,
            "a refused approval must not advance the epoch"
        );
        assert_eq!(commit_count(&fx), 0);
        assert!(!map_has(&fx, MEMBER_LIST_KEY, &peer));
    }

    #[tokio::test]
    async fn approve_without_a_key_package_changes_nothing() {
        let fx = fixture();
        let peer = "12D3KooWNeverRegistered".to_string();
        let sig = sign(&fx.admin, &format!("add:{peer}:member:owner:alice"));

        let resp = approve_member(
            axum::extract::State(fx.daemon.clone()),
            axum::extract::Path(CIRCLE.into()),
            axum::Json(ApproveMemberRequest {
                peer_id: peer.clone(),
                role: None,
                owner: "alice".into(),
                admin_signature: sig,
                agents: None,
            }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(epoch(&fx), 0);
        assert_eq!(commit_count(&fx), 0);
    }

    // ── Removal ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn removal_tombstones_the_peer_and_clears_its_records() {
        let fx = fixture();
        let peer = candidate(&fx, "alice");
        let added = approve_member(
            axum::extract::State(fx.daemon.clone()),
            axum::extract::Path(CIRCLE.into()),
            axum::Json(ApproveMemberRequest {
                peer_id: peer.clone(),
                role: None,
                owner: "alice".into(),
                admin_signature: sign(&fx.admin, &format!("add:{peer}:member:owner:alice")),
                agents: None,
            }),
        )
        .await
        .into_response();
        assert_eq!(added.status(), StatusCode::OK);
        assert!(map_has(&fx, MEMBER_LIST_KEY, &peer));
        let after_add = epoch(&fx);

        let resp = remove_member(
            axum::extract::State(fx.daemon.clone()),
            axum::extract::Path(CIRCLE.into()),
            axum::Json(RemoveMemberRequest {
                peer_id: peer.clone(),
                admin_signature: sign(&fx.admin, &format!("remove:{peer}")),
            }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::OK);

        assert!(map_has(&fx, MLS_REMOVED_KEY, &peer), "tombstone written");
        assert!(
            !map_has(&fx, MEMBER_LIST_KEY, &peer),
            "member entry cleared"
        );
        assert!(
            !map_has(&fx, MLS_KEY_PACKAGES_KEY, &peer),
            "key package cleared"
        );
        // The removal commit must reach the remaining members too.
        assert!(epoch(&fx) > after_add, "removal must advance the epoch");
        assert_eq!(
            commit_count(&fx),
            2,
            "add and remove commits both published"
        );
    }

    #[tokio::test]
    async fn removal_with_a_bad_signature_changes_nothing() {
        let fx = fixture();
        let peer = candidate(&fx, "alice");

        let resp = remove_member(
            axum::extract::State(fx.daemon.clone()),
            axum::extract::Path(CIRCLE.into()),
            axum::Json(RemoveMemberRequest {
                peer_id: peer.clone(),
                admin_signature: sign(&fx.admin, "remove:a-different-peer"),
            }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert!(
            !map_has(&fx, MLS_REMOVED_KEY, &peer),
            "no tombstone on refusal"
        );
    }

    // ── Grants ───────────────────────────────────────────────────────────────

    #[test]
    fn a_grant_from_the_admin_is_well_formed() {
        let fx = fixture();
        let g = grant(&fx);
        let as_invite = crate::invite::InviteGrant {
            inviter_pubkey_hex: g.inviter_pubkey_hex.clone(),
            nonce: g.nonce.clone(),
            sig: g.sig.clone(),
        };
        assert!(crate::invite::verify_grant(CIRCLE, &as_invite, g.expires_at).is_ok());
    }
}
