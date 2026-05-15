use chrono::Utc;
use libp2p::PeerId;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use yrs::{Any, Map, Out, Transact};

use crate::control::{AgentStatus, Presence, PRESENCE_KEY};
use crate::state::AppState;

/// Derive a stable, human-readable agent ID from the peer_id.
/// Do not include hostname here: hostnames can change or be unavailable, but a
/// peer should keep one presence identity across sessions.
pub fn local_agent_id(peer_id: &PeerId) -> String {
    let peer_str = peer_id.to_string();
    let short = &peer_str[peer_str.len().saturating_sub(8)..];
    format!("peer-{short}")
}

fn peer_suffix(agent_id: &str) -> &str {
    agent_id.rsplit_once('-').map(|(_, suffix)| suffix).unwrap_or(agent_id)
}

fn remove_legacy_presence_keys(state: &AppState, agent_id: &str) {
    let suffix = peer_suffix(agent_id);
    let map = state.control.get_or_insert_map(PRESENCE_KEY);
    let stale_keys: Vec<String> = {
        let txn = state.control.transact();
        map.iter(&txn)
            .map(|(key, _)| key.to_string())
            .filter(|key| key != agent_id && peer_suffix(key) == suffix)
            .collect()
    };
    if stale_keys.is_empty() {
        return;
    }
    let mut txn = state.control.transact_mut();
    for key in stale_keys {
        map.remove(&mut txn, key.as_str());
    }
}

fn read_presence(state: &AppState, agent_id: &str) -> Option<Presence> {
    let map = state.control.get_or_insert_map(PRESENCE_KEY);
    let txn = state.control.transact();
    match map.get(&txn, agent_id) {
        Some(Out::Any(Any::String(s))) => serde_json::from_str::<Presence>(&s).ok(),
        _ => None,
    }
}

fn write_presence_with_file(
    state: &AppState,
    agent_id: &str,
    status: AgentStatus,
    current_file: Option<String>,
) {
    let presence = Presence {
        agent_id: agent_id.to_string(),
        status,
        last_seen: Utc::now(),
        current_file,
    };
    let Ok(json) = serde_json::to_string(&presence) else { return };
    remove_legacy_presence_keys(state, agent_id);
    let map = state.control.get_or_insert_map(PRESENCE_KEY);
    let mut txn = state.control.transact_mut();
    map.insert(&mut txn, agent_id, json.as_str());
}

/// Write or refresh the local presence entry in the control doc.
fn write_presence(state: &AppState, agent_id: &str, status: AgentStatus) {
    let current_file = read_presence(state, agent_id).and_then(|p| p.current_file);
    write_presence_with_file(state, agent_id, status, current_file);
}

pub fn set_current_file(state: &AppState, current_file: Option<String>) {
    let status = read_presence(state, &state.agent_id)
        .map(|p| p.status)
        .unwrap_or(AgentStatus::Online);
    write_presence_with_file(state, &state.agent_id, status, current_file);
}

pub fn clear_current_file_if_matches(state: &AppState, current_file: &str) {
    let Some(presence) = read_presence(state, &state.agent_id) else { return };
    if presence.current_file.as_deref() == Some(current_file) {
        write_presence_with_file(state, &state.agent_id, presence.status, None);
    }
}

/// Write the initial presence entry and spawn the 30-second heartbeat task.
pub fn spawn_presence(state: AppState, agent_id: String, token: CancellationToken) {
    write_presence(&state, &agent_id, AgentStatus::Online);
    tracing::info!("[presence] online as '{agent_id}'");

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        interval.tick().await; // consume the immediate first tick
        loop {
            tokio::select! {
                _ = token.cancelled() => break,
                _ = interval.tick() => write_presence(&state, &agent_id, AgentStatus::Online),
            }
        }
        // Mark offline on clean shutdown
        write_presence_with_file(&state, &agent_id, AgentStatus::Offline, None);
    });
}
