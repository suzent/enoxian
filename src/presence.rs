use chrono::Utc;
use libp2p::PeerId;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use yrs::{Any, Map, Out, Transact};

use crate::control::{AgentStatus, Presence, PRESENCE_KEY};
use crate::state::AppState;

/// Derive a stable, human-readable agent ID from a custom agent name and peer_id.
///
/// Resolution order:
///   1. `enoxian_AGENT_ID` env var — explicit override (`codex`, `cursor`, `alice`, …)
///   2. System hostname — auto-detected, stripped of `.local` (macOS) / domain suffixes
///   3. `"device"` — last-resort fallback if hostname is unavailable
///
/// The peer suffix (-XXXXXXXX) keeps names unique across machines so two
/// `MacBook-Pro` devices appear as `MacBook-Pro-Kj4R` and `MacBook-Pro-Ab9F`.
pub fn local_agent_id(peer_id: &PeerId) -> String {
    let peer_str = peer_id.to_string();
    let short = &peer_str[peer_str.len().saturating_sub(8)..];
    let agent = std::env::var("enoxian_AGENT_ID")
        .ok()
        .map(|s| sanitize_agent_name(&s))
        .filter(|s| !s.is_empty())
        .or_else(|| {
            hostname_candidates()
                .into_iter()
                .next()
                .map(|h| sanitize_agent_name(&strip_domain_suffix(&h)))
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "device".to_string());
    if agent.ends_with(short) {
        agent
    } else {
        format!("{agent}-{short}")
    }
}

/// Strip trailing domain/mDNS suffixes so `MacBook-Pro.local` → `MacBook-Pro`.
fn strip_domain_suffix(hostname: &str) -> String {
    hostname
        .split('.')
        .next()
        .unwrap_or(hostname)
        .to_string()
}

fn sanitize_agent_name(raw: &str) -> String {
    raw.trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn peer_suffix(agent_id: &str) -> &str {
    agent_id.rsplit_once('-').map(|(_, suffix)| suffix).unwrap_or(agent_id)
}

fn hostname_candidates() -> Vec<String> {
    let mut names = Vec::new();
    for key in ["COMPUTERNAME", "HOSTNAME"] {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim();
            if !value.is_empty() {
                names.push(value.to_string());
            }
        }
    }
    if let Some(value) = std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        names.push(value);
    }
    names.sort();
    names.dedup();
    names
}

fn is_legacy_presence_id(agent_id: &str, suffix: &str) -> bool {
    let Some((prefix, id_suffix)) = agent_id.rsplit_once('-') else {
        return false;
    };
    if id_suffix != suffix {
        return false;
    }
    prefix == "unknown"
        || prefix == "peer"
        || prefix.ends_with(".local")
        || hostname_candidates().iter().any(|host| prefix == host)
}

fn remove_legacy_presence_keys(state: &AppState, agent_id: &str) {
    let suffix = peer_suffix(agent_id);
    let map = state.control.get_or_insert_map(PRESENCE_KEY);
    let stale_keys: Vec<String> = {
        let txn = state.control.transact();
        map.iter(&txn)
            .map(|(key, _)| key.to_string())
            .filter(|key| key != agent_id && is_legacy_presence_id(key, suffix))
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

/// Immediately mark a remote peer offline (called when their P2P connection drops).
/// Safe to call from any peer — the write is idempotent and CRDT-merged.
pub fn write_offline(state: &AppState, agent_id: &str) {
    write_presence_with_file(state, agent_id, AgentStatus::Offline, None);
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
