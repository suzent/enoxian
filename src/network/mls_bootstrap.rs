//! Minimal MLS delivery stream used before content encryption is available.
//!
//! Only public membership material, Welcomes and MLS commits cross this stream.
//! The stream is still protected by libp2p Noise and the circle transport PSK.
//! Workspace/control content uses the MLS-encrypted v2 protocols instead.

use anyhow::{Context, Result};
use libp2p::{PeerId, Stream, StreamProtocol};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tracing::warn;
use yrs::{Array, Map, Out, ReadTxn, Transact, WriteTxn};

use crate::control::{
    MlsCommitEntry, MEMBER_LIST_KEY, MLS_COMMITS_KEY, MLS_KEY_PACKAGES_KEY, MLS_OWNER_CLAIMS_KEY,
    MLS_PENDING_KEY, MLS_REMOVED_KEY, MLS_WELCOMES_KEY,
};
use crate::state::AppState;

pub const PROTOCOL: StreamProtocol = StreamProtocol::new("/enoxian/mls-bootstrap/1.0.0");
const MAX_FRAME: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Snapshot {
    circle_id: String,
    sender_peer_id: String,
    key_packages: Vec<(String, String)>,
    owner_claims: Vec<(String, String)>,
    pending: Vec<(String, String)>,
    members: Vec<(String, String)>,
    removed: Vec<(String, String)>,
    welcome: Option<String>,
    commits: Vec<MlsCommitEntry>,
}

/// Contention retry budget for the control doc.
///
/// This is the plaintext recovery path out of an MLS epoch deadlock: once two
/// devices are on different epochs, neither can decrypt the other's CRDT
/// frames, so the commits needed to catch up can only arrive here. Giving up
/// on a momentarily busy control doc makes that deadlock permanent — and the
/// control doc is the busiest doc in a circle, so "momentarily busy" is the
/// normal case rather than the exception.
const CONTENTION_RETRIES: u32 = 10;
const CONTENTION_BACKOFF: std::time::Duration = std::time::Duration::from_millis(50);

fn map_strings_in<T: ReadTxn>(txn: &T, key: &str) -> Vec<(String, String)> {
    let Some(map) = txn.get_map(key) else {
        return Vec::new();
    };
    let mut values: Vec<_> = map
        .iter(txn)
        .filter_map(|(key, value)| match value {
            Out::Any(yrs::Any::String(value)) => Some((key.to_string(), value.to_string())),
            _ => None,
        })
        .collect();
    values.sort_by(|a, b| a.0.cmp(&b.0));
    values
}

/// Build the snapshot from a single read transaction.
///
/// Previously each field took its own transaction — seven acquisitions per
/// snapshot, every one an independent chance to abort the whole bootstrap.
/// One transaction is both cheaper and a consistent view of the group state.
fn try_snapshot(state: &AppState, receiver: &PeerId) -> Option<Snapshot> {
    let txn = state.control.try_transact().ok()?;
    let mut commits: Vec<MlsCommitEntry> = txn
        .get_array(MLS_COMMITS_KEY)
        .into_iter()
        .flat_map(|commits| commits.iter(&txn))
        .filter_map(|value| match value {
            Out::Any(yrs::Any::String(value)) => serde_json::from_str(&value).ok(),
            _ => None,
        })
        .collect();
    commits.sort_by_key(|entry: &MlsCommitEntry| entry.epoch);
    let receiver = receiver.to_string();
    Some(Snapshot {
        circle_id: state.circle_id.clone(),
        sender_peer_id: state.peer_id.clone(),
        key_packages: map_strings_in(&txn, MLS_KEY_PACKAGES_KEY),
        owner_claims: map_strings_in(&txn, MLS_OWNER_CLAIMS_KEY),
        pending: map_strings_in(&txn, MLS_PENDING_KEY),
        members: map_strings_in(&txn, MEMBER_LIST_KEY),
        removed: map_strings_in(&txn, MLS_REMOVED_KEY),
        welcome: map_strings_in(&txn, MLS_WELCOMES_KEY)
            .into_iter()
            .find(|(peer, _)| peer == &receiver)
            .map(|(_, value)| value),
        commits,
    })
}

async fn snapshot(state: &AppState, receiver: &PeerId) -> Result<Snapshot> {
    for attempt in 0..CONTENTION_RETRIES {
        if let Some(snapshot) = try_snapshot(state, receiver) {
            return Ok(snapshot);
        }
        tokio::time::sleep(CONTENTION_BACKOFF * (attempt + 1)).await;
    }
    anyhow::bail!("circle state busy after retries; could not build MLS bootstrap snapshot")
}

/// Merge every incoming map under one write transaction, retrying on contention.
async fn merge_maps(state: &AppState, groups: &[(&str, &[(String, String)])]) -> Result<()> {
    for attempt in 0..CONTENTION_RETRIES {
        {
            if let Ok(mut txn) = state.control.try_transact_mut_with("p2p") {
                for (key, entries) in groups {
                    let map = txn.get_or_insert_map(*key);
                    for (entry_key, value) in entries.iter() {
                        let unchanged = map
                            .get(&txn, entry_key.as_str())
                            .is_some_and(|current| current.to_string(&txn) == *value);
                        if !unchanged {
                            map.insert(&mut txn, entry_key.as_str(), value.as_str());
                        }
                    }
                }
                return Ok(());
            }
        }
        tokio::time::sleep(CONTENTION_BACKOFF * (attempt + 1)).await;
    }
    anyhow::bail!("circle state busy after retries; MLS bootstrap merge deferred")
}

async fn apply_snapshot(state: &AppState, peer: PeerId, incoming: Snapshot) -> Result<()> {
    anyhow::ensure!(
        incoming.circle_id == state.circle_id,
        "bootstrap circle mismatch"
    );
    anyhow::ensure!(
        incoming.sender_peer_id == peer.to_string(),
        "bootstrap peer mismatch"
    );
    merge_maps(
        state,
        &[
            (MLS_KEY_PACKAGES_KEY, &incoming.key_packages),
            (MLS_OWNER_CLAIMS_KEY, &incoming.owner_claims),
            (MLS_PENDING_KEY, &incoming.pending),
            (MEMBER_LIST_KEY, &incoming.members),
            (MLS_REMOVED_KEY, &incoming.removed),
        ],
    )
    .await?;

    if let Some(welcome) = incoming.welcome {
        crate::lifecycle::consume_welcome(welcome.clone(), state.mls.clone(), state.clone()).await;
        merge_maps(
            state,
            &[(MLS_WELCOMES_KEY, &[(state.peer_id.clone(), welcome)][..])],
        )
        .await?;
    }

    for entry in incoming.commits {
        crate::lifecycle::apply_commit_entry(entry.clone(), state.mls.clone(), state.clone()).await;
        let json = serde_json::to_string(&entry)?;
        append_commit(state, json).await?;
    }
    Ok(())
}

/// Append a commit to the shared log if it is not already there, retrying on
/// contention. Dropping a commit here is what strands a peer on an old epoch.
async fn append_commit(state: &AppState, json: String) -> Result<()> {
    for attempt in 0..CONTENTION_RETRIES {
        {
            if let Ok(mut txn) = state.control.try_transact_mut_with("p2p") {
                let commits_ref = txn.get_or_insert_array(MLS_COMMITS_KEY);
                let exists = commits_ref
                    .iter(&txn)
                    .any(|value| value.to_string(&txn) == json);
                if !exists {
                    commits_ref.push_back(&mut txn, json);
                }
                return Ok(());
            }
        }
        tokio::time::sleep(CONTENTION_BACKOFF * (attempt + 1)).await;
    }
    anyhow::bail!("circle state busy after retries; MLS commit not recorded")
}

async fn write_snapshot<W: AsyncWriteExt + Unpin>(writer: &mut W, value: &Snapshot) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    anyhow::ensure!(bytes.len() <= MAX_FRAME, "MLS bootstrap frame too large");
    writer
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .await?;
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

async fn read_snapshot<R: AsyncReadExt + Unpin>(reader: &mut R) -> Result<Snapshot> {
    let mut len = [0; 4];
    reader.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    anyhow::ensure!(len <= MAX_FRAME, "MLS bootstrap frame too large: {len}");
    let mut bytes = vec![0; len];
    reader.read_exact(&mut bytes).await?;
    serde_json::from_slice(&bytes).context("decoding MLS bootstrap snapshot")
}

pub async fn run(peer: PeerId, stream: Stream, state: AppState, initiator: bool) {
    if let Err(error) = run_inner(peer, stream, &state, initiator).await {
        warn!("[mls-bootstrap] {peer}: bootstrap ended: {error}");
    }
}

async fn run_inner(peer: PeerId, stream: Stream, state: &AppState, initiator: bool) -> Result<()> {
    let (mut reader, mut writer) = tokio::io::split(stream.compat());
    let first = snapshot(state, &peer).await?;
    let remote = if initiator {
        write_snapshot(&mut writer, &first).await?;
        read_snapshot(&mut reader).await?
    } else {
        let remote = read_snapshot(&mut reader).await?;
        write_snapshot(&mut writer, &first).await?;
        remote
    };
    apply_snapshot(state, peer, remote).await?;

    let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_sent = serde_json::to_vec(&first)?;
    loop {
        tokio::select! {
            incoming = read_snapshot(&mut reader) => apply_snapshot(state, peer, incoming?).await?,
            _ = interval.tick() => {
                let next = snapshot(state, &peer).await?;
                let encoded = serde_json::to_vec(&next)?;
                if encoded != last_sent {
                    write_snapshot(&mut writer, &next).await?;
                    last_sent = encoded;
                }
            }
        }
    }
}
