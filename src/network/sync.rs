/// P2P sync handler — runs the y-sync protocol over a libp2p Stream.
///
/// Protocol (deadlock-free):
///   Initiator: sends [count][SyncStep1...] → reads [SyncStep2...][count_r][SyncStep1_r...] → sends [SyncStep2_r...]
///   Responder: reads [count][SyncStep1...] → sends [SyncStep2...][count_r][SyncStep1_r...] → reads [SyncStep2_r...]
///   Both: enter continuous Update/SyncStep1 exchange (see IncomingEvent).
///
/// Framing: [4-byte path len][path UTF-8][4-byte data len][y-sync bytes]
use anyhow::{Context, Result};
use libp2p::{PeerId, Stream, StreamProtocol};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tracing::{debug, info, warn};
use yrs::sync::protocol::{Message, SyncMessage};
use yrs::updates::decoder::{Decode, DecoderV1};
use yrs::updates::encoder::{Encode, Encoder, EncoderV1};
use yrs::{
    encoding::read::Cursor, Any, GetString, Map, Out, ReadTxn, StateVector, Transact, Update,
};

use crate::control::MLS_REMOVED_KEY;
use crate::state::AppState;

pub const PROTOCOL: StreamProtocol = StreamProtocol::new("/enoxian/sync/1.0.0");
const AWARENESS_PATH_PREFIX: &str = "\0awareness/";
const DELETE_PATH_PREFIX: &str = "\0delete/";
const REVOKED_PATH: &str = "\0revoked";
const SESSION_PATH: &str = "\0session";
const SESSION_HELLO_MAGIC: &[u8] = b"enoxian-sync-session-v2\0";

#[derive(Debug, Clone, PartialEq, Eq)]
struct PeerSession {
    circle_id: Option<String>,
    session_id: u64,
    you_are_removed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionHello {
    circle_id: String,
    session_id: u64,
    #[serde(default)]
    you_are_removed: bool,
}

// ── Wire helpers ──────────────────────────────────────────────────────────────

async fn write_frame<W: AsyncWriteExt + Unpin>(w: &mut W, path: &str, data: &[u8]) -> Result<()> {
    let pb = path.as_bytes();
    w.write_all(&(pb.len() as u32).to_be_bytes()).await?;
    w.write_all(pb).await?;
    w.write_all(&(data.len() as u32).to_be_bytes()).await?;
    w.write_all(data).await?;
    w.flush().await?;
    Ok(())
}

async fn read_frame<R: AsyncReadExt + Unpin>(r: &mut R) -> Result<(String, Vec<u8>)> {
    let mut u32buf = [0u8; 4];
    r.read_exact(&mut u32buf).await?;
    let plen = u32::from_be_bytes(u32buf) as usize;
    let mut pbuf = vec![0u8; plen];
    r.read_exact(&mut pbuf).await?;
    let path = String::from_utf8(pbuf)?;

    r.read_exact(&mut u32buf).await?;
    let dlen = u32::from_be_bytes(u32buf) as usize;
    let mut data = vec![0u8; dlen];
    r.read_exact(&mut data).await?;
    Ok((path, data))
}

async fn write_u32<W: AsyncWriteExt + Unpin>(w: &mut W, n: u32) -> Result<()> {
    w.write_all(&n.to_be_bytes()).await?;
    w.flush().await?;
    Ok(())
}

async fn write_awareness_frame<W: AsyncWriteExt + Unpin>(
    w: &mut W,
    path: &str,
    data: &[u8],
) -> Result<()> {
    let path = format!("{AWARENESS_PATH_PREFIX}{path}");
    write_frame(w, &path, data).await
}

async fn flush_pending_awareness<W: AsyncWriteExt + Unpin>(
    w: &mut W,
    rx: &mut tokio::sync::broadcast::Receiver<(String, Vec<u8>)>,
    peer_id: PeerId,
) -> Result<()> {
    loop {
        match rx.try_recv() {
            Ok((path, data)) => write_awareness_frame(w, &path, &data).await?,
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Lagged(n)) => {
                debug!("[sync] lagged {n} awareness updates to {peer_id}");
                continue;
            }
            Err(TryRecvError::Closed) => break,
        }
    }
    Ok(())
}

async fn read_u32<R: AsyncReadExt + Unpin>(r: &mut R) -> Result<u32> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf).await?;
    Ok(u32::from_be_bytes(buf))
}

fn encode_sync(msg: Message) -> Vec<u8> {
    let mut enc = EncoderV1::new();
    msg.encode(&mut enc);
    enc.to_vec()
}

fn encode_session_hello(state: &AppState, you_are_removed: bool) -> Result<Vec<u8>> {
    let hello = SessionHello {
        circle_id: state.circle_id.clone(),
        session_id: state.session_id,
        you_are_removed,
    };
    let mut bytes = Vec::with_capacity(SESSION_HELLO_MAGIC.len() + 96);
    bytes.extend_from_slice(SESSION_HELLO_MAGIC);
    bytes.extend_from_slice(&serde_json::to_vec(&hello)?);
    Ok(bytes)
}

fn decode_session_hello(data: &[u8]) -> Result<PeerSession> {
    if let Some(json) = data.strip_prefix(SESSION_HELLO_MAGIC) {
        let hello: SessionHello =
            serde_json::from_slice(json).context("invalid sync session hello")?;
        return Ok(PeerSession {
            circle_id: Some(hello.circle_id),
            session_id: hello.session_id,
            you_are_removed: hello.you_are_removed,
        });
    }

    // Backward compatibility for peers that only sent an 8-byte session_id.
    if data.len() == 8 {
        return Ok(PeerSession {
            circle_id: None,
            session_id: u64::from_be_bytes(data.try_into().unwrap()),
            you_are_removed: false,
        });
    }

    Err(anyhow::anyhow!("invalid sync session frame"))
}

async fn write_session_hello<W: AsyncWriteExt + Unpin>(
    w: &mut W,
    state: &AppState,
    you_are_removed: bool,
) -> Result<()> {
    let hello = encode_session_hello(state, you_are_removed)?;
    write_frame(w, SESSION_PATH, &hello).await
}

async fn read_session_hello<R: AsyncReadExt + Unpin>(r: &mut R) -> Result<PeerSession> {
    let (path, data) = read_frame(r).await?;
    if path != SESSION_PATH {
        return Err(anyhow::anyhow!("expected sync session frame, got {path:?}"));
    }
    decode_session_hello(&data)
}

// ── Events from the reader task to the writer loop ───────────────────────────

enum IncomingEvent {
    /// Peer sent an Update or SyncStep2 — apply locally
    Apply { path: String, raw_update: Vec<u8> },
    /// Peer sent SyncStep1 (they lagged and need our state) — send SyncStep2 back
    ResyncRequest { path: String, sv: Vec<u8> },
    /// Peer sent an ephemeral y-protocols awareness update for a file doc.
    Awareness { path: String, data: Vec<u8> },
    /// Peer deleted a file doc.
    Delete { path: String },
    /// The remote peer has revoked this device's Circle membership.
    Revoked,
    /// Stream closed
    Closed,
}

fn parse_frame(path: String, data: &[u8]) -> IncomingEvent {
    if path == REVOKED_PATH {
        return IncomingEvent::Revoked;
    }
    if let Some(doc_path) = path.strip_prefix(DELETE_PATH_PREFIX) {
        return IncomingEvent::Delete {
            path: doc_path.to_string(),
        };
    }

    if let Some(doc_path) = path.strip_prefix(AWARENESS_PATH_PREFIX) {
        return IncomingEvent::Awareness {
            path: doc_path.to_string(),
            data: data.to_vec(),
        };
    }

    let mut dec = DecoderV1::new(Cursor::new(data));
    match Message::decode(&mut dec) {
        Ok(Message::Sync(SyncMessage::SyncStep1(sv))) => IncomingEvent::ResyncRequest {
            path,
            sv: sv.encode_v1(),
        },
        Ok(Message::Sync(SyncMessage::SyncStep2(raw)))
        | Ok(Message::Sync(SyncMessage::Update(raw))) => IncomingEvent::Apply {
            path,
            raw_update: raw,
        },
        _ => IncomingEvent::Closed,
    }
}

// ── Apply an update to the local CRDT ────────────────────────────────────────

fn device_label_for_peer(state: &AppState, peer_id: &PeerId) -> Option<String> {
    use crate::control::{MemberEntry, MEMBER_LIST_KEY};
    use yrs::Map;
    let peer_str = peer_id.to_string();
    let map = state.control.get_or_insert_map(MEMBER_LIST_KEY);
    let txn = state.control.transact();
    match map.get(&txn, &peer_str) {
        Some(Out::Any(Any::String(s))) => {
            serde_json::from_str::<MemberEntry>(&s).ok().map(|e| e.device_label)
        }
        _ => None,
    }
}

fn apply_update(state: &AppState, path: &str, raw: &[u8], peer_id: PeerId) {
    let doc = if path == "__control__" {
        state.control.clone()
    } else {
        state.get_or_create_doc(path)
    };

    match Update::decode_v1(raw) {
        Ok(update) => {
            // Use "p2p" origin so the observer skips forwarding this back to all_updates,
            // preventing the update from echoing to the peer that just sent it.
            if let Err(e) = doc.transact_mut_with("p2p").apply_update(update) {
                warn!("[sync] apply_update for {path}: {e}");
                return;
            }
            if path != "__control__" {
                let author = device_label_for_peer(state, &peer_id);
                let state = state.clone();
                let path = path.to_string();
                tokio::spawn(async move {
                    crate::store::fs::flush_to_disk(&state, &path, author).await;
                });
            }
        }
        Err(e) => warn!("[sync] decode_v1 for {path}: {e}"),
    }
}

fn apply_awareness(state: &AppState, path: &str, data: Vec<u8>) {
    let tx = state.awareness_sender(path);
    let _ = tx.send(data);
}

async fn apply_delete(state: &AppState, path: &str) {
    state.remove_doc(path);
    crate::store::crdt::delete(&state.workspace, path).await;

    let full_path = state
        .workspace
        .join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
    let _ = tokio::fs::remove_file(full_path).await;

    let _ = state.events.send(crate::control::CircleEvent::FileDeleted {
        path: path.to_string(),
    });
}

/// Encode the full CRDT state of a doc as an Update message.
/// Sending this is equivalent to "here is everything I have" — safe to apply
/// at any time because CRDT merges are idempotent.
fn full_state_update(state: &AppState, path: &str) -> Vec<u8> {
    let doc = if path == "__control__" {
        state.control.clone()
    } else {
        state.get_or_create_doc(path)
    };
    let empty_sv = StateVector::default();
    let full_diff = doc.transact().encode_diff_v1(&empty_sv);
    encode_sync(Message::Sync(SyncMessage::Update(full_diff)))
}

fn all_doc_paths(state: &AppState) -> Vec<String> {
    let mut paths: Vec<String> = state.docs.iter().map(|e| e.key().clone()).collect();
    paths.insert(0, "__control__".to_string());
    paths
}

/// Returns true if both sides have operations the other side doesn't have —
/// i.e., both edited independently while offline (genuine divergence).
fn sv_has_divergence(our_sv_bytes: &[u8], their_sv_bytes: &[u8]) -> bool {
    let our_sv = StateVector::decode_v1(our_sv_bytes).unwrap_or_default();
    let their_sv = StateVector::decode_v1(their_sv_bytes).unwrap_or_default();
    // They have a client or clock we haven't seen yet.
    let they_have_new = their_sv
        .iter()
        .any(|(client, &their_clock)| our_sv.get(client) < their_clock);
    // We have a client or clock they haven't seen yet.
    let we_have_new = our_sv
        .iter()
        .any(|(client, &our_clock)| their_sv.get(client) < our_clock);
    they_have_new && we_have_new
}

/// Write a conflict copy of `rel_path` to `<rel_path>.conflict.<agent_id>`.
/// The content is the pre-merge version — captured before the CRDT merge is applied.
async fn write_conflict_copy(state: &AppState, rel_path: &str, content: &str) {
    let conflict_rel = crate::store::conflicts::conflict_rel_path(rel_path, &state.agent_id);
    let full_path = state
        .workspace
        .join(conflict_rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    if let Some(parent) = full_path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let _ = tokio::fs::write(&full_path, content).await;
    info!("[sync] conflict copy saved: {rel_path} → {conflict_rel}");
}

// ── Main entry point ──────────────────────────────────────────────────────────

fn sync_revoked(state: &AppState, peer_id: &PeerId) -> bool {
    state.is_self_removed() || state.is_peer_removed(&peer_id.to_string())
}

fn mark_self_removed(state: &AppState) {
    use crate::control::{CircleEvent, MEMBER_LIST_KEY};

    let removed = state.control.get_or_insert_map(MLS_REMOVED_KEY);
    let members = state.control.get_or_insert_map(MEMBER_LIST_KEY);
    let removed_at = chrono::Utc::now().to_rfc3339();
    {
        let mut txn = state.control.transact_mut();
        removed.insert(&mut txn, state.peer_id.as_str(), removed_at.as_str());
        members.remove(&mut txn, state.peer_id.as_str());
    }
    crate::presence::write_offline(state, &state.agent_id);
    let _ = state.events.send(CircleEvent::MemberRemoved {
        peer_id: state.peer_id.clone(),
    });
}

pub async fn run_sync(peer_id: PeerId, stream: Stream, state: AppState, is_initiator: bool) {
    if let Err(e) = sync_inner(peer_id, stream, &state, is_initiator).await {
        debug!("[sync] {peer_id}: {e}");
    }
}

async fn sync_inner(
    peer_id: PeerId,
    stream: Stream,
    state: &AppState,
    is_initiator: bool,
) -> Result<()> {
    // ── Membership gate ────────────────────────────────────────────────────────
    //
    // After PSK + Noise, a peer is cryptographically authenticated but not yet
    // authorised.  Before exchanging any CRDT data, check the tombstone set.
    //
    // Design rationale — why tombstone rather than allowlist:
    //
    //   "Not in member_list" is ambiguous: it could mean "fresh joiner" (allow)
    //   or "evicted peer" (reject).  Fresh joiners must be allowed to sync the
    //   control doc to deliver their KeyPackage to the admin — so checking for
    //   presence in member_list would break the join flow.
    //
    //   The tombstone set (mls_removed) is unambiguous: only peers that have been
    //   explicitly evicted via remove_member appear here.  Everyone else — fresh
    //   joiners, pending peers, members — is correctly allowed through.
    //
    // The transport PSK stays stable, so this persisted tombstone is the
    // authorization boundary. It is checked again during continuous exchange
    // so removal also closes streams that were already established.
    let compat = stream.compat();
    let (mut rx, mut tx) = tokio::io::split(compat);
    let remote_is_removed = state.is_peer_removed(&peer_id.to_string());

    let my_paths = all_doc_paths(state);
    let mut all_awareness_rx = state.all_awareness_updates.subscribe();

    // ── Session exchange ──────────────────────────────────────────────────────
    //
    // Both sides send their session_id before any CRDT frames.
    // Initiator sends first, then reads; responder reads first, then sends.
    // This gives us two pieces of information per reconnect:
    //   1. Their current session_id  — did they restart since we last talked?
    //   2. Our saved last_session_id for them — did WE restart since then?
    // Together these detect the dual-offline case needed for conflict detection.

    let peer_session: PeerSession;

    if is_initiator {
        write_session_hello(&mut tx, state, remote_is_removed).await?;
        peer_session = read_session_hello(&mut rx).await?;
    } else {
        peer_session = read_session_hello(&mut rx).await?;
        write_session_hello(&mut tx, state, remote_is_removed).await?;
    }

    if peer_session.you_are_removed {
        mark_self_removed(state);
        warn!("[sync] this device was removed from circle {}", state.circle_id);
        return Err(anyhow::anyhow!("this device was removed from circle"));
    }
    if remote_is_removed || state.is_self_removed() {
        warn!("[sync] rejected {peer_id}: explicitly removed from this circle");
        return Err(anyhow::anyhow!("peer rejected: removed from circle"));
    }

    if let Some(remote_circle_id) = &peer_session.circle_id {
        if remote_circle_id != &state.circle_id {
            warn!(
                "[sync] rejected {peer_id}: circle mismatch (remote {remote_circle_id}, local {})",
                state.circle_id
            );
            return Err(anyhow::anyhow!(
                "peer rejected: circle mismatch (remote {remote_circle_id}, local {})",
                state.circle_id
            ));
        }
    } else {
        warn!(
            "[sync] {peer_id}: legacy session hello without circle_id; allowing sync for compatibility"
        );
    }

    let now = chrono::Utc::now().timestamp();
    let peer_id_str = peer_id.to_string();
    crate::store::session::record_peer(
        &state.circle_dir,
        &peer_id_str,
        peer_session.session_id,
        now,
    )
    .await;
    tracing::info!(
        "[sync] {peer_id}: session {} (ours: {}, circle {})",
        peer_session.session_id,
        state.session_id,
        state.circle_id
    );

    // ── Pre-merge snapshot ────────────────────────────────────────────────────
    //
    // Capture each file doc's state vector and text content BEFORE any peer
    // updates are applied. The state vector is used to detect divergence; the
    // content is used as the conflict copy source. This must happen before the
    // handshake so that the initiator (which applies SyncStep2 before receiving
    // the responder's SyncStep1) still has access to the pre-merge state.

    let pre_merge: HashMap<String, (Vec<u8>, String)> = state
        .docs
        .iter()
        .map(|entry| {
            let path = entry.key().clone();
            let doc = entry.value();
            let sv = doc.transact().state_vector().encode_v1();
            let text = doc.get_or_insert_text(&*path);
            let content = text.get_string(&doc.transact());
            (path, (sv, content))
        })
        .collect();

    // ── Handshake ─────────────────────────────────────────────────────────────

    if is_initiator {
        // Send our SyncStep1 messages
        write_u32(&mut tx, my_paths.len() as u32).await?;
        for path in &my_paths {
            let doc = if path == "__control__" {
                state.control.clone()
            } else {
                state.get_or_create_doc(path)
            };
            let sv = doc.transact().state_vector();
            write_frame(
                &mut tx,
                path,
                &encode_sync(Message::Sync(SyncMessage::SyncStep1(sv))),
            )
            .await?;
        }

        // Read SyncStep2 replies (one per our SyncStep1)
        for _ in 0..my_paths.len() {
            let (path, data) = read_frame(&mut rx).await?;
            if let IncomingEvent::Apply { raw_update, .. } = parse_frame(path.clone(), &data) {
                apply_update(state, &path, &raw_update, peer_id);
            }
        }

        // Read responder's SyncStep1 count + messages, send SyncStep2 for each.
        // SyncStep2 was already applied above, so we use the pre-merge snapshot
        // for divergence detection rather than the (already merged) current state.
        let their_count = read_u32(&mut rx).await? as usize;
        for _ in 0..their_count {
            let (path, data) = read_frame(&mut rx).await?;
            if let IncomingEvent::ResyncRequest {
                sv: their_sv_bytes, ..
            } = parse_frame(path.clone(), &data)
            {
                // Conflict detection: compare responder's sv against our PRE-MERGE sv.
                if path != "__control__" {
                    if let Some((our_pre_sv, our_pre_content)) = pre_merge.get(&path) {
                        if sv_has_divergence(our_pre_sv, &their_sv_bytes) {
                            write_conflict_copy(state, &path, our_pre_content).await;
                        }
                    }
                }
                let sv = StateVector::decode_v1(&their_sv_bytes)?;
                let doc = if path == "__control__" {
                    state.control.clone()
                } else {
                    state.get_or_create_doc(&path)
                };
                let diff = doc.transact().encode_diff_v1(&sv);
                write_frame(
                    &mut tx,
                    &path,
                    &encode_sync(Message::Sync(SyncMessage::SyncStep2(diff))),
                )
                .await?;
            }
        }
    } else {
        // Read initiator's SyncStep1 messages, send SyncStep2 for each
        let their_count = read_u32(&mut rx).await? as usize;
        for _ in 0..their_count {
            let (path, data) = read_frame(&mut rx).await?;
            if let IncomingEvent::ResyncRequest {
                sv: their_sv_bytes, ..
            } = parse_frame(path.clone(), &data)
            {
                // Conflict detection: check before any update is applied to our CRDT.
                if path != "__control__" {
                    if let Some((our_sv_bytes, our_content)) = pre_merge.get(&path) {
                        if sv_has_divergence(our_sv_bytes, &their_sv_bytes) {
                            write_conflict_copy(state, &path, our_content).await;
                        }
                    }
                }
                let sv = StateVector::decode_v1(&their_sv_bytes)?;
                let doc = if path == "__control__" {
                    state.control.clone()
                } else {
                    state.get_or_create_doc(&path)
                };
                let diff = doc.transact().encode_diff_v1(&sv);
                write_frame(
                    &mut tx,
                    &path,
                    &encode_sync(Message::Sync(SyncMessage::SyncStep2(diff))),
                )
                .await?;
            }
        }

        // Send our own SyncStep1 messages
        write_u32(&mut tx, my_paths.len() as u32).await?;
        for path in &my_paths {
            let doc = if path == "__control__" {
                state.control.clone()
            } else {
                state.get_or_create_doc(path)
            };
            let sv = doc.transact().state_vector();
            write_frame(
                &mut tx,
                path,
                &encode_sync(Message::Sync(SyncMessage::SyncStep1(sv))),
            )
            .await?;
        }

        // Read initiator's SyncStep2 replies
        for _ in 0..my_paths.len() {
            let (path, data) = read_frame(&mut rx).await?;
            if let IncomingEvent::Apply { raw_update, .. } = parse_frame(path.clone(), &data) {
                apply_update(state, &path, &raw_update, peer_id);
            }
        }
    }

    tracing::info!("[sync] handshake complete with {peer_id}");

    if state.is_peer_removed(&peer_id.to_string()) {
        let _ = write_frame(&mut tx, REVOKED_PATH, &[]).await;
        return Err(anyhow::anyhow!("peer removed during sync handshake"));
    }
    if state.is_self_removed() {
        return Err(anyhow::anyhow!("this device was removed during sync handshake"));
    }

    // Subscribe to awareness before the handshake, then flush anything that
    // happened while CRDT/session/conflict setup was running. Otherwise cursor
    // frames produced during the handshake are lost because awareness is
    // ephemeral and broadcast receivers only see events after subscription.
    flush_pending_awareness(&mut tx, &mut all_awareness_rx, peer_id).await?;

    // ── Post-handshake catch-up ───────────────────────────────────────────────
    //
    // The handshake only covers docs that were open on BOTH sides at the moment
    // it ran. If one side has docs the other doesn't know about, those are missed.
    // Fix: immediately push our full CRDT state for every doc we hold as Update
    // messages — the peer applies them idempotently via the continuous exchange
    // reader. Both sides do this, so convergence is guaranteed regardless of the
    // initial asymmetry.
    for path in all_doc_paths(state) {
        if sync_revoked(state, &peer_id) {
            if state.is_peer_removed(&peer_id.to_string()) {
                let _ = write_frame(&mut tx, REVOKED_PATH, &[]).await;
            }
            return Err(anyhow::anyhow!("peer removed during sync catch-up"));
        }
        let msg = full_state_update(state, &path);
        write_frame(&mut tx, &path, &msg).await?;
    }
    flush_pending_awareness(&mut tx, &mut all_awareness_rx, peer_id).await?;

    // ── Continuous exchange ───────────────────────────────────────────────────
    //
    // The reader runs in a dedicated task so read_frame is never cancelled
    // mid-frame (which would corrupt the stream). It forwards events to the
    // writer loop via an mpsc channel.
    //
    // The writer loop selects between:
    //   - incoming events from the reader (apply update, awareness, or resync)
    //   - local CRDT updates to forward to peer (from all_updates broadcast)
    //   - local awareness updates to forward to peer (from all_awareness_updates)
    //   - local file deletions to forward to peer (from all_deletes)
    //
    // Lag handling: if all_updates overflows and we miss broadcasts, we send
    // our full CRDT state for every doc. The peer applies it idempotently.

    let (evt_tx, mut evt_rx) = tokio::sync::mpsc::channel::<IncomingEvent>(256);
    let peer_str = peer_id.to_string();

    tokio::spawn(async move {
        loop {
            match read_frame(&mut rx).await {
                Ok((path, data)) => {
                    if evt_tx.send(parse_frame(path, &data)).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    debug!("[sync] reader closed ({peer_str}): {e}");
                    let _ = evt_tx.send(IncomingEvent::Closed).await;
                    break;
                }
            }
        }
    });

    let mut all_rx = state.all_updates.subscribe();
    let mut all_deletes_rx = state.all_deletes.subscribe();
    let mut circle_events_rx = state.events.subscribe();
    let remote_peer_id = peer_id.to_string();

    loop {
        if sync_revoked(state, &peer_id) {
            warn!("[sync] closing stream to removed peer {peer_id}");
            break;
        }
        tokio::select! {
            event = circle_events_rx.recv() => {
                if matches!(
                    event,
                    Ok(crate::control::CircleEvent::MemberRemoved { peer_id: ref removed })
                        if removed == &remote_peer_id
                ) || sync_revoked(state, &peer_id) {
                    if state.is_peer_removed(&peer_id.to_string()) {
                        let _ = write_frame(&mut tx, REVOKED_PATH, &[]).await;
                    }
                    warn!("[sync] closing stream to removed peer {peer_id}");
                    break;
                }
            }

            // ── Incoming from peer ──────────────────────────────────────────
            Some(event) = evt_rx.recv() => {
                if sync_revoked(state, &peer_id) {
                    warn!("[sync] discarded frame from removed peer {peer_id}");
                    break;
                }
                match event {
                    IncomingEvent::Apply { path, raw_update } => {
                        apply_update(state, &path, &raw_update, peer_id);
                    }
                    IncomingEvent::ResyncRequest { path, sv } => {
                        // Peer lagged — send them our full state for this doc
                        let sv = StateVector::decode_v1(&sv)?;
                        let doc = if path == "__control__" { state.control.clone() } else { state.get_or_create_doc(&path) };
                        let diff = doc.transact().encode_diff_v1(&sv);
                        let step2 = encode_sync(Message::Sync(SyncMessage::SyncStep2(diff)));
                        write_frame(&mut tx, &path, &step2).await?;
                    }
                    IncomingEvent::Awareness { path, data } => {
                        apply_awareness(state, &path, data);
                    }
                    IncomingEvent::Delete { path } => {
                        apply_delete(state, &path).await;
                    }
                    IncomingEvent::Revoked => {
                        mark_self_removed(state);
                        warn!("[sync] membership revoked by {peer_id}");
                        break;
                    }
                    IncomingEvent::Closed => break,
                }
            }

            // ── Outgoing local updates ──────────────────────────────────────
            result = all_rx.recv() => {
                if sync_revoked(state, &peer_id) {
                    if state.is_peer_removed(&peer_id.to_string()) {
                        let _ = write_frame(&mut tx, REVOKED_PATH, &[]).await;
                    }
                    warn!("[sync] stopped forwarding updates to removed peer {peer_id}");
                    break;
                }
                match result {
                    Ok((path, raw)) => {
                        let msg = encode_sync(Message::Sync(SyncMessage::Update(raw)));
                        write_frame(&mut tx, &path, &msg).await?;
                    }
                    Err(RecvError::Lagged(n)) => {
                        // We dropped n updates. Send our full CRDT state for every
                        // doc so the peer is guaranteed to converge. CRDT merges
                        // are idempotent so re-sending existing state is safe.
                        warn!("[sync] lagged {n} updates to {peer_id} — sending full state");
                        for path in all_doc_paths(state) {
                            let msg = full_state_update(state, &path);
                            write_frame(&mut tx, &path, &msg).await?;
                        }
                    }
                    Err(RecvError::Closed) => break,
                }
            }

            // ── Outgoing local awareness updates ───────────────────────────
            result = all_awareness_rx.recv() => {
                if sync_revoked(state, &peer_id) {
                    break;
                }
                match result {
                    Ok((path, data)) => {
                        write_awareness_frame(&mut tx, &path, &data).await?;
                    }
                    Err(RecvError::Lagged(n)) => {
                        // Awareness is ephemeral. If we drop cursor frames, the
                        // next local movement/selection update will repair state.
                        debug!("[sync] lagged {n} awareness updates to {peer_id}");
                    }
                    Err(RecvError::Closed) => break,
                }
            }

            // ── Outgoing local file deletions ─────────────────────────────
            result = all_deletes_rx.recv() => {
                if sync_revoked(state, &peer_id) {
                    break;
                }
                match result {
                    Ok(path) => {
                        let path = format!("{DELETE_PATH_PREFIX}{path}");
                        write_frame(&mut tx, &path, &[]).await?;
                    }
                    Err(RecvError::Lagged(n)) => {
                        warn!("[sync] lagged {n} delete events to {peer_id}");
                    }
                    Err(RecvError::Closed) => break,
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_legacy_session_id() {
        let session_id = 42_u64;
        assert_eq!(
            decode_session_hello(&session_id.to_be_bytes()).unwrap(),
            PeerSession {
                circle_id: None,
                session_id,
                you_are_removed: false,
            }
        );
    }

    #[test]
    fn decodes_session_hello_with_circle_id() {
        let hello = SessionHello {
            circle_id: "circle-123".to_string(),
            session_id: 99,
            you_are_removed: true,
        };
        let mut bytes = SESSION_HELLO_MAGIC.to_vec();
        bytes.extend_from_slice(&serde_json::to_vec(&hello).unwrap());

        assert_eq!(
            decode_session_hello(&bytes).unwrap(),
            PeerSession {
                circle_id: Some("circle-123".to_string()),
                session_id: 99,
                you_are_removed: true,
            }
        );
    }

    #[test]
    fn parses_revocation_frame() {
        assert!(matches!(
            parse_frame(REVOKED_PATH.to_string(), &[]),
            IncomingEvent::Revoked
        ));
    }
}
