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
use tracing::debug;
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

fn map_strings(state: &AppState, key: &str) -> Result<Vec<(String, String)>> {
    let txn = state
        .control
        .try_transact()
        .map_err(|_| anyhow::anyhow!("circle state busy"))?;
    let Some(map) = txn.get_map(key) else {
        return Ok(Vec::new());
    };
    let mut values: Vec<_> = map
        .iter(&txn)
        .filter_map(|(key, value)| match value {
            Out::Any(yrs::Any::String(value)) => Some((key.to_string(), value.to_string())),
            _ => None,
        })
        .collect();
    values.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(values)
}

fn snapshot(state: &AppState, receiver: &PeerId) -> Result<Snapshot> {
    let welcomes = map_strings(state, MLS_WELCOMES_KEY)?;
    let txn = state
        .control
        .try_transact()
        .map_err(|_| anyhow::anyhow!("circle state busy"))?;
    let mut commits: Vec<_> = txn
        .get_array(MLS_COMMITS_KEY)
        .into_iter()
        .flat_map(|commits| commits.iter(&txn))
        .filter_map(|value| match value {
            Out::Any(yrs::Any::String(value)) => serde_json::from_str(&value).ok(),
            _ => None,
        })
        .collect();
    commits.sort_by_key(|entry: &MlsCommitEntry| entry.epoch);
    Ok(Snapshot {
        circle_id: state.circle_id.clone(),
        sender_peer_id: state.peer_id.clone(),
        key_packages: map_strings(state, MLS_KEY_PACKAGES_KEY)?,
        owner_claims: map_strings(state, MLS_OWNER_CLAIMS_KEY)?,
        pending: map_strings(state, MLS_PENDING_KEY)?,
        members: map_strings(state, MEMBER_LIST_KEY)?,
        removed: map_strings(state, MLS_REMOVED_KEY)?,
        welcome: welcomes
            .into_iter()
            .find(|(peer, _)| peer == &receiver.to_string())
            .map(|(_, value)| value),
        commits,
    })
}

fn merge_map(state: &AppState, key: &str, entries: &[(String, String)]) -> Result<()> {
    let mut txn = state
        .control
        .try_transact_mut_with("p2p")
        .map_err(|_| anyhow::anyhow!("circle state busy"))?;
    let map = txn.get_or_insert_map(key);
    for (entry_key, value) in entries {
        let unchanged = map
            .get(&txn, entry_key.as_str())
            .is_some_and(|current| current.to_string(&txn) == *value);
        if !unchanged {
            map.insert(&mut txn, entry_key.as_str(), value.as_str());
        }
    }
    Ok(())
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
    merge_map(state, MLS_KEY_PACKAGES_KEY, &incoming.key_packages)?;
    merge_map(state, MLS_OWNER_CLAIMS_KEY, &incoming.owner_claims)?;
    merge_map(state, MLS_PENDING_KEY, &incoming.pending)?;
    merge_map(state, MEMBER_LIST_KEY, &incoming.members)?;
    merge_map(state, MLS_REMOVED_KEY, &incoming.removed)?;

    if let Some(welcome) = incoming.welcome {
        crate::lifecycle::consume_welcome(welcome.clone(), state.mls.clone(), state.clone()).await;
        merge_map(state, MLS_WELCOMES_KEY, &[(state.peer_id.clone(), welcome)])?;
    }

    for entry in incoming.commits {
        crate::lifecycle::apply_commit_entry(entry.clone(), state.mls.clone(), state.clone()).await;
        let json = serde_json::to_string(&entry)?;
        let exists = {
            let txn = state
                .control
                .try_transact()
                .map_err(|_| anyhow::anyhow!("circle state busy"))?;
            txn.get_array(MLS_COMMITS_KEY)
                .into_iter()
                .flat_map(|commits| commits.iter(&txn))
                .any(|value| value.to_string(&txn) == json)
        };
        if !exists {
            let mut txn = state
                .control
                .try_transact_mut_with("p2p")
                .map_err(|_| anyhow::anyhow!("circle state busy"))?;
            let commits_ref = txn.get_or_insert_array(MLS_COMMITS_KEY);
            commits_ref.push_back(&mut txn, json);
        }
    }
    Ok(())
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
        debug!("[mls-bootstrap] {peer}: {error}");
    }
}

async fn run_inner(peer: PeerId, stream: Stream, state: &AppState, initiator: bool) -> Result<()> {
    let (mut reader, mut writer) = tokio::io::split(stream.compat());
    let first = snapshot(state, &peer)?;
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
                let next = snapshot(state, &peer)?;
                let encoded = serde_json::to_vec(&next)?;
                if encoded != last_sent {
                    write_snapshot(&mut writer, &next).await?;
                    last_sent = encoded;
                }
            }
        }
    }
}
