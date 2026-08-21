//! Encrypted live workspace event-log synchronization — `/enoxian/events/2.0.0`.
//!
//! Peers first reconcile immutable event ids, then keep the stream open and
//! forward newly appended events. Proposal-related events carry the existing
//! `ProposalBundle`, so metadata, snapshot manifests, and ordinary blobs arrive
//! with the decision instead of waiting for a reconnect.

use anyhow::{Context, Result};
use libp2p::{PeerId, Stream, StreamProtocol};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tracing::{debug, warn};

use crate::control::CircleEvent;
use crate::proposal::store::ProposalStore;
use crate::proposal::sync::ProposalBundle;
use crate::state::AppState;
use crate::workspace_event::{EventStore, WorkspaceEvent};

pub const PROTOCOL: StreamProtocol = StreamProtocol::new("/enoxian/events/2.0.0");
const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EventEnvelope {
    event: WorkspaceEvent,
    #[serde(skip_serializing_if = "Option::is_none")]
    proposal: Option<ProposalBundle>,
}

#[derive(Debug, Serialize, Deserialize)]
enum Msg {
    Have(Vec<String>),
    Want(Vec<String>),
    Event(Box<EventEnvelope>),
    EventsDone,
}

pub fn compute_wants(local: &[String], peer: &[String]) -> Vec<String> {
    let local: BTreeSet<&str> = local.iter().map(String::as_str).collect();
    peer.iter()
        .filter(|id| !local.contains(id.as_str()))
        .cloned()
        .collect()
}

fn envelope_for(events: &EventStore, proposals: &ProposalStore, id: &str) -> Option<EventEnvelope> {
    let event = events.load(id).ok()?;
    let proposal_id = event.kind.proposal_id().map(str::to_owned).or_else(|| {
        let snapshot_id = match &event.kind {
            crate::workspace_event::WorkspaceEventKind::SnapshotRecorded {
                snapshot_id, ..
            } => snapshot_id,
            _ => return None,
        };
        proposals
            .list_proposals()
            .into_iter()
            .find(|proposal| proposal.result_snapshot == *snapshot_id)
            .map(|proposal| proposal.id)
    });
    let proposal = proposal_id
        .and_then(|proposal_id| proposals.load_proposal(&proposal_id).ok())
        .and_then(|proposal| ProposalBundle::from_store(proposals, &proposal).ok());
    Some(EventEnvelope { event, proposal })
}

fn envelopes_for(
    events: &EventStore,
    proposals: &ProposalStore,
    ids: &[String],
) -> Vec<EventEnvelope> {
    ids.iter()
        .filter_map(|id| envelope_for(events, proposals, id))
        .collect()
}

fn apply_envelope(
    state: &AppState,
    events: &EventStore,
    proposals: &ProposalStore,
    envelope: &EventEnvelope,
) -> Result<bool> {
    if let Some(bundle) = &envelope.proposal {
        crate::proposal::validate_bundle_for_circle(bundle, &state.circle_id)?;
        bundle.apply_to_store(proposals)?;
    }
    let appended = events.append(&envelope.event)?;
    if appended {
        reconcile_proposals(state, events, proposals);
        let _ = state.events.send(CircleEvent::WorkspaceEventAppended {
            event_id: envelope.event.id.clone(),
        });
    }
    Ok(appended)
}

/// Make legacy mutable proposal records reflect the authoritative event-log
/// materialization. This keeps old API/UI readers working during the M15
/// migration while decisions themselves converge through immutable events.
pub fn reconcile_proposals(
    state: &AppState,
    events: &EventStore,
    proposals: &ProposalStore,
) -> usize {
    let materialized = events.materialize();
    let mut changed = 0;
    for (id, event_proposal) in materialized.proposals {
        let Ok(mut proposal) = proposals.load_proposal(&id) else {
            continue;
        };
        if proposal.status == event_proposal.status {
            continue;
        }
        proposal.set_status(event_proposal.status);
        if proposals.save_proposal(&proposal).is_ok() {
            changed += 1;
            let status = serde_json::to_value(proposal.status)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default();
            let _ = state.events.send(CircleEvent::ProposalUpdated {
                proposal_id: id,
                status,
            });
        }
    }
    changed
}

async fn write_msg<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    state: &AppState,
    msg: &Msg,
) -> Result<()> {
    let bytes = serde_json::to_vec(msg)?;
    let bytes = crate::network::content_crypto::seal(
        state,
        crate::network::content_crypto::FrameKind::WorkspaceEvent,
        &bytes,
    )
    .await?;
    anyhow::ensure!(
        bytes.len() <= MAX_FRAME_BYTES,
        "workspace event frame too large"
    );
    writer
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .await?;
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

async fn read_msg<R: AsyncReadExt + Unpin>(reader: &mut R, state: &AppState) -> Result<Msg> {
    let mut len = [0; 4];
    reader.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    anyhow::ensure!(
        len <= MAX_FRAME_BYTES,
        "workspace event frame too large: {len}"
    );
    let mut bytes = vec![0; len];
    reader.read_exact(&mut bytes).await?;
    let plaintext = crate::network::content_crypto::open(
        state,
        crate::network::content_crypto::FrameKind::WorkspaceEvent,
        &bytes,
    )
    .await?;
    serde_json::from_slice(&plaintext).context("decoding workspace event message")
}

async fn write_event_batch<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    state: &AppState,
    events: Vec<EventEnvelope>,
) -> Result<()> {
    for event in events {
        write_msg(writer, state, &Msg::Event(Box::new(event))).await?;
    }
    write_msg(writer, state, &Msg::EventsDone).await
}

async fn read_event_batch<R: AsyncReadExt + Unpin>(
    reader: &mut R,
    state: &AppState,
) -> Result<Vec<EventEnvelope>> {
    let mut events = Vec::new();
    loop {
        match read_msg(reader, state).await? {
            Msg::Event(event) => events.push(*event),
            Msg::EventsDone => return Ok(events),
            _ => anyhow::bail!("unexpected message in initial event batch"),
        }
    }
}

fn ensure_peer_authorized(state: &AppState, peer_id: &PeerId) -> Result<()> {
    anyhow::ensure!(
        !state.is_self_removed() && !state.is_peer_removed(&peer_id.to_string()),
        "peer removed from circle"
    );
    Ok(())
}

pub async fn run(peer_id: PeerId, stream: Stream, state: AppState, is_initiator: bool) {
    if let Err(error) = run_inner(peer_id, stream, &state, is_initiator).await {
        debug!("[event-sync] {peer_id}: {error}");
    }
}

async fn run_inner(
    peer_id: PeerId,
    stream: Stream,
    state: &AppState,
    is_initiator: bool,
) -> Result<()> {
    ensure_peer_authorized(state, &peer_id)?;
    let events = EventStore::open(&state.workspace, state.circle_id.clone())?;
    let proposals = ProposalStore::open(&state.workspace)?;
    let (mut reader, mut writer) = tokio::io::split(stream.compat());
    // Subscribe before taking the HAVE snapshot so events appended during the
    // initial handshake remain queued for the live phase.
    let mut local_events = state.events.subscribe();

    let local_ids = events.ids();
    let peer_ids = if is_initiator {
        write_msg(&mut writer, state, &Msg::Have(local_ids.clone())).await?;
        match read_msg(&mut reader, state).await? {
            Msg::Have(ids) => ids,
            _ => anyhow::bail!("expected event HAVE"),
        }
    } else {
        let ids = match read_msg(&mut reader, state).await? {
            Msg::Have(ids) => ids,
            _ => anyhow::bail!("expected event HAVE"),
        };
        write_msg(&mut writer, state, &Msg::Have(local_ids.clone())).await?;
        ids
    };
    ensure_peer_authorized(state, &peer_id)?;

    let want = compute_wants(&local_ids, &peer_ids);
    let peer_want = if is_initiator {
        write_msg(&mut writer, state, &Msg::Want(want.clone())).await?;
        match read_msg(&mut reader, state).await? {
            Msg::Want(ids) => ids,
            _ => anyhow::bail!("expected event WANT"),
        }
    } else {
        let ids = match read_msg(&mut reader, state).await? {
            Msg::Want(ids) => ids,
            _ => anyhow::bail!("expected event WANT"),
        };
        write_msg(&mut writer, state, &Msg::Want(want.clone())).await?;
        ids
    };
    ensure_peer_authorized(state, &peer_id)?;

    let outgoing = envelopes_for(&events, &proposals, &peer_want);
    let incoming = if is_initiator {
        write_event_batch(&mut writer, state, outgoing).await?;
        read_event_batch(&mut reader, state).await?
    } else {
        let incoming = read_event_batch(&mut reader, state).await?;
        write_event_batch(&mut writer, state, outgoing).await?;
        incoming
    };

    let mut peer_known: BTreeSet<String> = peer_ids.into_iter().collect();
    for envelope in incoming {
        ensure_peer_authorized(state, &peer_id)?;
        peer_known.insert(envelope.event.id.clone());
        if let Err(error) = apply_envelope(state, &events, &proposals, &envelope) {
            warn!("[event-sync] rejected {}: {error}", envelope.event.id);
        }
    }
    reconcile_proposals(state, &events, &proposals);

    // Keep the stream alive. New local and forwarded events are pushed without
    // requiring a disconnect/reconnect cycle.
    // Own the reader in one task. Cancelling `read_exact` halfway through a
    // frame would lose framing alignment, which a direct `select!` between the
    // socket read and local event notifications could do.
    let (incoming_tx, mut incoming_rx) = tokio::sync::mpsc::channel(16);
    let reader_state = state.clone();
    let _reader_task = AbortOnDrop(tokio::spawn(async move {
        loop {
            let message = read_msg(&mut reader, &reader_state).await;
            let stop = message.is_err();
            if incoming_tx.send(message).await.is_err() || stop {
                break;
            }
        }
    }));
    loop {
        tokio::select! {
            message = incoming_rx.recv() => {
                ensure_peer_authorized(state, &peer_id)?;
                match message.context("event-sync reader stopped")?? {
                    Msg::Event(envelope) => {
                        peer_known.insert(envelope.event.id.clone());
                        if let Err(error) = apply_envelope(state, &events, &proposals, &envelope) {
                            warn!("[event-sync] rejected live {}: {error}", envelope.event.id);
                        }
                    }
                    _ => anyhow::bail!("unexpected event-sync message after reconciliation"),
                }
            }
            event = local_events.recv() => match event {
                Ok(CircleEvent::WorkspaceEventAppended { event_id }) => {
                    if peer_known.contains(&event_id) {
                        continue;
                    }
                    ensure_peer_authorized(state, &peer_id)?;
                    if let Some(envelope) = envelope_for(&events, &proposals, &event_id) {
                        write_msg(&mut writer, state, &Msg::Event(Box::new(envelope))).await?;
                        peer_known.insert(event_id);
                    }
                }
                Ok(CircleEvent::MemberRemoved { peer_id: removed })
                    if removed == peer_id.to_string() || removed == state.peer_id =>
                {
                    anyhow::bail!("peer removed during event sync");
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    // The disk log is authoritative. Recover every event the
                    // bounded notification channel may have dropped.
                    for event_id in events.ids() {
                        if peer_known.contains(&event_id) {
                            continue;
                        }
                        ensure_peer_authorized(state, &peer_id)?;
                        if let Some(envelope) = envelope_for(&events, &proposals, &event_id) {
                            write_msg(&mut writer, state, &Msg::Event(Box::new(envelope))).await?;
                            peer_known.insert(event_id);
                        }
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return Ok(()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proposal::model::{Proposal, ProposalStatus};
    use crate::proposal::snapshot::{FileEntry, Snapshot};
    use crate::workspace_event::WorkspaceEventKind;
    use std::collections::BTreeMap;

    #[test]
    fn event_wants_are_set_difference() {
        assert_eq!(
            compute_wants(
                &["a".into(), "b".into()],
                &["b".into(), "c".into(), "d".into()]
            ),
            vec!["c".to_string(), "d".to_string()]
        );
    }

    fn wire_event(lamport: u64) -> WorkspaceEvent {
        WorkspaceEvent {
            schema_version: crate::workspace_event::SCHEMA_VERSION,
            id: uuid::Uuid::new_v4().to_string(),
            circle_id: "circle".into(),
            parents: Vec::new(),
            lamport,
            origin_peer_id: "peer".into(),
            origin_device: "device".into(),
            created_at: chrono::Utc::now(),
            kind: WorkspaceEventKind::SnapshotRecorded {
                snapshot_id: uuid::Uuid::new_v4().to_string(),
                parent_snapshot: None,
            },
        }
    }

    #[test]
    fn initial_event_batch_uses_bounded_individual_messages() {
        let events = vec![
            EventEnvelope {
                event: wire_event(1),
                proposal: None,
            },
            EventEnvelope {
                event: wire_event(2),
                proposal: None,
            },
        ];
        for event in events {
            let bytes = serde_json::to_vec(&Msg::Event(Box::new(event))).unwrap();
            assert!(bytes.len() <= MAX_FRAME_BYTES);
        }
    }

    fn snapshot(store: &ProposalStore, path: &str, content: &[u8]) -> Snapshot {
        let hash = store.blobs.put(content).unwrap();
        let mut files = BTreeMap::new();
        files.insert(
            path.into(),
            FileEntry {
                hash,
                size: content.len() as u64,
            },
        );
        let snapshot = Snapshot::new(files);
        store.save_snapshot(&snapshot).unwrap();
        snapshot
    }

    #[test]
    fn two_peer_event_union_converges_status_and_conflict_metadata() {
        let a_dir = tempfile::tempdir().unwrap();
        let b_dir = tempfile::tempdir().unwrap();
        let a_proposals = ProposalStore::open(a_dir.path()).unwrap();
        let b_proposals = ProposalStore::open(b_dir.path()).unwrap();
        let a_events = EventStore::open(a_dir.path(), "circle").unwrap();
        let b_events = EventStore::open(b_dir.path(), "circle").unwrap();

        let base = snapshot(&a_proposals, "a.txt", b"base");
        let result = snapshot(&a_proposals, "a.txt", b"result");
        let mut proposal = Proposal::ambient(
            "circle".into(),
            base.id.clone(),
            result.id.clone(),
            vec!["a.txt".into()],
        );
        proposal.status = ProposalStatus::Pending;
        a_proposals.save_proposal(&proposal).unwrap();
        let created = a_events
            .append_local(
                "peer-a",
                "a",
                WorkspaceEventKind::ProposalCreated {
                    proposal_id: proposal.id.clone(),
                    base_snapshot: base.id,
                    result_snapshot: result.id.clone(),
                    changed_paths: proposal.changed_paths.clone(),
                    initial_status: ProposalStatus::Pending,
                },
            )
            .unwrap();

        // Initial A -> B transfer carries both event and proposal bundle.
        let envelope = envelope_for(&a_events, &a_proposals, &created.id).unwrap();
        envelope
            .proposal
            .as_ref()
            .unwrap()
            .apply_to_store(&b_proposals)
            .unwrap();
        b_events.append(&envelope.event).unwrap();

        // Peers decide concurrently. Rejected outranks accepted independent of
        // event arrival order, and conflict paths are part of the same log.
        let accepted = a_events
            .append_local(
                "peer-a",
                "a",
                WorkspaceEventKind::ProposalStatusChanged {
                    proposal_id: proposal.id.clone(),
                    status: ProposalStatus::Accepted,
                    materialized_snapshot: result.clone().id,
                },
            )
            .unwrap();
        let rejected_snapshot = Snapshot::new(BTreeMap::new());
        b_proposals.save_snapshot(&rejected_snapshot).unwrap();
        let rejected = b_events
            .append_local(
                "peer-b",
                "b",
                WorkspaceEventKind::ProposalRejected {
                    proposal_id: proposal.id.clone(),
                    materialized_snapshot: rejected_snapshot.id,
                },
            )
            .unwrap();
        let conflict = b_events
            .append_local(
                "peer-b",
                "b",
                WorkspaceEventKind::ConflictDetected {
                    proposal_id: proposal.id.clone(),
                    paths: vec!["a.txt".into()],
                },
            )
            .unwrap();

        for event in [&accepted, &rejected, &conflict] {
            a_events.append(event).unwrap_or(false);
            b_events.append(event).unwrap_or(false);
        }
        let a = a_events.materialize();
        let b = b_events.materialize();
        assert_eq!(a, b);
        assert_eq!(a.proposals[&proposal.id].status, ProposalStatus::Rejected);
        assert_eq!(a.proposals[&proposal.id].conflicts, vec!["a.txt"]);
    }
}
