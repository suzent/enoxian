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
    // This exchange is not MLS-encrypted, which makes it the only place a
    // peer's Circle can be established before any key agreement. The sync
    // handshake carries `circle_id` too, but inside the encrypted envelope —
    // so a peer from another Circle fails decryption there long before the
    // check runs, and could never be identified. Record it here instead, so
    // the swarm stops dialing it.
    if incoming.circle_id != state.circle_id {
        if state.mark_foreign_peer(&peer.to_string()) {
            warn!(
                "[mls-bootstrap] {peer} belongs to circle {}; suppressing further dials from circle {}",
                incoming.circle_id, state.circle_id
            );
        }
        anyhow::bail!("bootstrap circle mismatch");
    }
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

struct SnapshotReader {
    incoming: tokio::sync::mpsc::Receiver<Result<Snapshot>>,
    task: tokio::task::JoinHandle<()>,
}

impl SnapshotReader {
    fn spawn<R: AsyncReadExt + Unpin + Send + 'static>(mut reader: R) -> Self {
        let (sender, incoming) = tokio::sync::mpsc::channel(16);
        let task = tokio::spawn(async move {
            loop {
                let result = read_snapshot(&mut reader).await;
                let stop = result.is_err();
                if sender.send(result).await.is_err() || stop {
                    break;
                }
            }
        });
        Self { incoming, task }
    }

    async fn recv(&mut self) -> Result<Snapshot> {
        self.incoming
            .recv()
            .await
            .context("MLS bootstrap reader stopped")?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use std::{
        pin::Pin,
        task::{Context as TaskContext, Poll},
        time::Duration,
    };
    use tokio::io::{AsyncRead, ReadBuf};

    struct CountReads {
        inner: tokio::io::DuplexStream,
        consumed: Arc<AtomicUsize>,
    }

    impl AsyncRead for CountReads {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut TaskContext<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            let before = buf.filled().len();
            let result = Pin::new(&mut self.inner).poll_read(cx, buf);
            self.consumed
                .fetch_add(buf.filled().len() - before, Ordering::SeqCst);
            result
        }
    }

    fn snapshot_value(circle: &str) -> Snapshot {
        Snapshot {
            circle_id: circle.into(),
            sender_peer_id: "peer".into(),
            key_packages: vec![],
            owner_claims: vec![],
            pending: vec![],
            members: vec![],
            removed: vec![],
            welcome: None,
            commits: vec![],
        }
    }

    async fn fragmented_snapshot(split: usize) {
        let (mut writer, reader) = tokio::io::duplex(4096);
        let consumed = Arc::new(AtomicUsize::new(0));
        let mut reader = SnapshotReader::spawn(CountReads {
            inner: reader,
            consumed: consumed.clone(),
        });
        let value = snapshot_value("enoxian");
        let payload = serde_json::to_vec(&value).unwrap();
        let mut frame = (payload.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(&payload);
        writer.write_all(&frame[..split]).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while consumed.load(Ordering::SeqCst) < split {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        // Force several timer wins after the socket has consumed a partial
        // header/payload. Each win cancels recv, but must not cancel the read.
        let mut ticks = tokio::time::interval(Duration::from_millis(1));
        for _ in 0..3 {
            tokio::select! {
                result = reader.recv() => panic!("partial frame completed: {result:?}"),
                _ = ticks.tick() => {}
            }
        }
        writer.write_all(&frame[split..]).await.unwrap();
        write_snapshot(&mut writer, &snapshot_value("second"))
            .await
            .unwrap();
        let first = tokio::time::timeout(Duration::from_secs(2), reader.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.circle_id, "enoxian");
        let second = tokio::time::timeout(Duration::from_secs(2), reader.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second.circle_id, "second");
        drop(writer);
        assert!(tokio::time::timeout(Duration::from_secs(2), reader.recv())
            .await
            .unwrap()
            .is_err());
    }

    #[tokio::test]
    async fn partial_header_survives_timer_ticks() {
        fragmented_snapshot(2).await;
    }

    #[tokio::test]
    async fn partial_payload_survives_timer_ticks() {
        fragmented_snapshot(8).await;
    }

    #[tokio::test]
    async fn truncated_frame_and_oversized_frame_report_errors() {
        for header in [32u32, MAX_FRAME as u32 + 1] {
            let (mut writer, stream) = tokio::io::duplex(64);
            let mut reader = SnapshotReader::spawn(stream);
            writer.write_all(&header.to_be_bytes()).await.unwrap();
            drop(writer);
            let error = tokio::time::timeout(Duration::from_secs(2), reader.recv())
                .await
                .unwrap()
                .unwrap_err();
            if header > MAX_FRAME as u32 {
                assert!(error.to_string().contains("frame too large"));
            } else {
                assert!(error.to_string().contains("early eof"));
            }
        }
    }

    #[tokio::test]
    async fn dropping_reader_closes_pending_socket() {
        let (mut writer, stream) = tokio::io::duplex(64);
        let reader = SnapshotReader::spawn(stream);
        drop(reader);
        let mut byte = [0];
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), writer.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            0
        );
    }
}

impl Drop for SnapshotReader {
    fn drop(&mut self) {
        self.task.abort();
    }
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
    // read_exact is not cancellation-safe: a timer tick after consuming part
    // of a frame would discard its progress and interpret JSON as a length.
    // Only the channel receive competes with ticks; the socket reader persists.
    let mut incoming = SnapshotReader::spawn(reader);
    loop {
        tokio::select! {
            remote = incoming.recv() => apply_snapshot(state, peer, remote?).await?,
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
