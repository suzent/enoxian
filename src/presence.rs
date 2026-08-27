use chrono::Utc;
use libp2p::PeerId;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use yrs::{Any, Map, Out, ReadTxn, Transact, WriteTxn};

use crate::control::{AgentStatus, Presence, PRESENCE_KEY};
use crate::state::AppState;

/// Derive a stable, human-readable agent ID from the device identity and peer_id.
///
/// Resolution order:
///   1. `ENOXIAN_AGENT_ID` env var — explicit override (`codex`, `cursor`, `alice`, …)
///   2. Device identity `user_handle` (if set, e.g. "suzy")
///   3. Device identity `device_label` (e.g. "macbook-pro")
///   4. System hostname — auto-detected, stripped of `.local` / domain suffixes
///   5. `"device"` — last-resort fallback
///
/// The peer suffix (-XXXXXXXX) keeps names unique across machines so two
/// `macbook-pro` devices appear as `macbook-pro-Kj4R` and `macbook-pro-Ab9F`.
pub fn local_agent_id(peer_id: &PeerId) -> String {
    let peer_str = peer_id.to_string();
    let short = &peer_str[peer_str.len().saturating_sub(8)..];
    let agent = std::env::var("ENOXIAN_AGENT_ID")
        .or_else(|_| std::env::var("enoxian_AGENT_ID"))
        .ok()
        .map(|s| sanitize_agent_name(&s))
        .filter(|s| !s.is_empty())
        .or_else(|| {
            // Prefer the device identity display name (user_handle or device_label).
            crate::identity::read_identity_display()
                .map(|(label, handle)| sanitize_agent_name(handle.as_deref().unwrap_or(&label)))
                .filter(|s| !s.is_empty())
        })
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
    hostname.split('.').next().unwrap_or(hostname).to_string()
}

fn sanitize_agent_name(raw: &str) -> String {
    raw.trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn peer_suffix(agent_id: &str) -> &str {
    agent_id
        .rsplit_once('-')
        .map(|(_, suffix)| suffix)
        .unwrap_or(agent_id)
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

fn read_presence(state: &AppState, agent_id: &str) -> Option<Presence> {
    let txn = state.control.try_transact().ok()?;
    let map = txn.get_map(PRESENCE_KEY)?;
    match map.get(&txn, agent_id) {
        Some(Out::Any(Any::String(s))) => serde_json::from_str::<Presence>(&s).ok(),
        _ => None,
    }
}

/// Returns false if the control doc was busy and the entry was not written.
fn write_presence_with_file(
    state: &AppState,
    agent_id: &str,
    status: AgentStatus,
    current_file: Option<String>,
) -> bool {
    let presence = Presence {
        agent_id: agent_id.to_string(),
        status,
        last_seen: Utc::now(),
        current_file,
        peer_id: state.peer_id.clone(),
    };
    let Ok(json) = serde_json::to_string(&presence) else {
        return false;
    };
    let mut txn = match state.control.try_transact_mut() {
        Ok(txn) => txn,
        Err(_) => return false,
    };
    let map = txn.get_or_insert_map(PRESENCE_KEY);
    let suffix = peer_suffix(agent_id);
    let stale_keys = map
        .iter(&txn)
        .map(|(key, _)| key.to_string())
        .filter(|key| key != agent_id && is_legacy_presence_id(key, suffix))
        .collect::<Vec<_>>();
    for key in stale_keys {
        map.remove(&mut txn, key.as_str());
    }
    map.insert(&mut txn, agent_id, json.as_str());
    true
}

/// Write or refresh the local presence entry in the control doc.
fn write_presence(state: &AppState, agent_id: &str, status: AgentStatus) -> bool {
    let current_file = read_presence(state, agent_id).and_then(|p| p.current_file);
    write_presence_with_file(state, agent_id, status, current_file)
}

/// Heartbeat retry budget. See [`heartbeat`].
const PRESENCE_RETRIES: u32 = 6;
const PRESENCE_BACKOFF: Duration = Duration::from_millis(50);

/// Refresh the local presence entry, retrying while the control doc is busy.
///
/// A dropped heartbeat is not cosmetic. Presence staleness is derived purely
/// from `last_seen`, and the tick is 30s — so silently skipping a write makes a
/// perfectly healthy local agent show as stale to every peer, and to `enox who`
/// on this machine. The control doc is also the most contended doc in a circle,
/// which is exactly when the skip fires.
async fn heartbeat(state: &AppState, agent_id: &str, status: AgentStatus) {
    for attempt in 0..PRESENCE_RETRIES {
        if write_presence(state, agent_id, status.clone()) {
            return;
        }
        tokio::time::sleep(PRESENCE_BACKOFF * (attempt + 1)).await;
    }
    tracing::warn!("[presence] control doc busy after retries; {agent_id} heartbeat skipped");
}

pub fn set_current_file(state: &AppState, current_file: Option<String>) {
    let status = read_presence(state, &state.agent_id)
        .map(|p| p.status)
        .unwrap_or(AgentStatus::Online);
    write_presence_with_file(state, &state.agent_id, status, current_file);
}

pub fn clear_current_file_if_matches(state: &AppState, current_file: &str) {
    let Some(presence) = read_presence(state, &state.agent_id) else {
        return;
    };
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
                _ = interval.tick() => heartbeat(&state, &agent_id, AgentStatus::Online).await,
            }
        }
        // Mark offline on clean shutdown
        write_presence_with_file(&state, &agent_id, AgentStatus::Offline, None);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use crate::{config::JoinPolicy, mls};
    use std::path::PathBuf;

    fn test_state() -> AppState {
        AppState::new(
            "circle".into(),
            "Circle".into(),
            PathBuf::new(),
            PathBuf::new(),
            String::new(),
            "suzy-local".into(),
            1,
            "peer-local".into(),
            JoinPolicy::Manual,
            "owner".into(),
            mls::new_mls_state(mls::MlsIdentity::generate("peer-local").unwrap(), None),
        )
    }

    /// Regression: a heartbeat that loses the race for the control doc must
    /// report the failure rather than silently claiming success, so the caller
    /// can retry. Presence staleness is computed from `last_seen` alone, so a
    /// dropped write shows a live agent as offline to every peer.
    #[test]
    fn contended_write_reports_failure_then_succeeds_once_free() {
        let state = test_state();

        {
            let _held = state.control.try_transact_mut().unwrap();
            assert!(
                !write_presence(&state, "suzy-local", AgentStatus::Online),
                "a contended control doc must report the write as failed"
            );
        }

        assert!(
            write_presence(&state, "suzy-local", AgentStatus::Online),
            "the same write must succeed once the doc is free"
        );
        assert!(read_presence(&state, "suzy-local").is_some());
    }
}
