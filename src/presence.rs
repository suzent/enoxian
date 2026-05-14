use chrono::Utc;
use libp2p::PeerId;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use yrs::{Map, Transact};

use crate::control::{AgentStatus, Presence, PRESENCE_KEY};
use crate::state::AppState;

/// Derive a human-readable agent ID from the hostname and peer_id.
/// Format: `<hostname>-<last8 of peer_id>`
pub fn local_agent_id(peer_id: &PeerId) -> String {
    let host = hostname();
    let peer_str = peer_id.to_string();
    let short = &peer_str[peer_str.len().saturating_sub(8)..];
    format!("{host}-{short}")
}

fn hostname() -> String {
    // COMPUTERNAME on Windows; HOSTNAME on Linux (often set); neither on macOS by default.
    if let Ok(h) = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")) {
        return h;
    }
    // Fall back to the `hostname` command — always available on macOS and Linux.
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Write or refresh the local presence entry in the control doc.
fn write_presence(state: &AppState, agent_id: &str, status: AgentStatus) {
    let presence = Presence {
        agent_id: agent_id.to_string(),
        status,
        last_seen: Utc::now(),
        current_file: None,
    };
    let Ok(json) = serde_json::to_string(&presence) else { return };
    let map = state.control.get_or_insert_map(PRESENCE_KEY);
    let mut txn = state.control.transact_mut();
    map.insert(&mut txn, agent_id, json.as_str());
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
        write_presence(&state, &agent_id, AgentStatus::Offline);
    });
}
