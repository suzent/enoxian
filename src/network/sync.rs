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
    encoding::read::Cursor, Any, Doc, GetString, Map, Out, ReadTxn, StateVector, Transact, Update,
};

use crate::control::MLS_REMOVED_KEY;
use crate::state::AppState;

pub const PROTOCOL: StreamProtocol = StreamProtocol::new("/enoxian/sync/2.0.0");
const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

const AWARENESS_PATH_PREFIX: &str = "\0awareness/";
const DELETE_PATH_PREFIX: &str = "\0delete/";
const REVOKED_PATH: &str = "\0revoked";
const SESSION_PATH: &str = "\0session";
const SESSION_HELLO_MAGIC: &[u8] = b"enoxian-sync-session-v2\0";

/// Doc-lock contention retry budget.
///
/// A `try_transact` failure means another writer holds the doc for the duration
/// of one update application — microseconds to low milliseconds. It is never a
/// reason to abort a sync, but we must not block the runtime waiting either
/// (that is what caused the WebUI lockups these `try_*` calls were introduced
/// for). So: retry with a short async backoff, which yields to the scheduler.
///
/// This matters most in large circles. Every one of these calls sits in a loop
/// over all docs, so a per-doc failure probability compounds across the whole
/// workspace — a circle with hundreds of files hits a busy doc on almost every
/// pass, while a three-file circle almost never does.
const CONTENTION_RETRIES: u32 = 8;
const CONTENTION_BACKOFF: std::time::Duration = std::time::Duration::from_millis(25);

/// Read a doc's state vector, retrying while another writer holds the lock.
///
/// On exhaustion returns an empty state vector — "I know nothing about this
/// doc" — which makes the peer send us its full state. Costs bandwidth, but
/// converges, and keeps the handshake's frame count intact (the reader expects
/// exactly one frame per path, so an early return here would desync framing).
async fn state_vector_retry(doc: &Doc, path: &str) -> StateVector {
    for attempt in 0..CONTENTION_RETRIES {
        if let Ok(txn) = doc.try_transact() {
            return txn.state_vector();
        }
        tokio::time::sleep(CONTENTION_BACKOFF * (attempt + 1)).await;
    }
    warn!("[sync] {path}: state busy after retries; requesting full state from peer");
    StateVector::default()
}

/// Encode a doc's diff against `sv`, retrying while another writer holds the lock.
///
/// On exhaustion returns an empty diff rather than aborting. The doc is not
/// dropped from the sync: the continuous exchange forwards later edits, the
/// peer can ask for it explicitly with a resync request, and a lagged broadcast
/// still triggers a full-state resend.
async fn encode_diff_retry(doc: &Doc, sv: &StateVector, path: &str) -> Vec<u8> {
    for attempt in 0..CONTENTION_RETRIES {
        if let Ok(txn) = doc.try_transact() {
            return txn.encode_diff_v1(sv);
        }
        tokio::time::sleep(CONTENTION_BACKOFF * (attempt + 1)).await;
    }
    warn!("[sync] {path}: state busy after retries; sending empty diff (catch-up will resend)");
    Vec::new()
}

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

async fn write_frame<W: AsyncWriteExt + Unpin>(
    w: &mut W,
    state: &AppState,
    path: &str,
    data: &[u8],
) -> Result<()> {
    let pb = path.as_bytes();
    let mut plaintext = Vec::with_capacity(8 + pb.len() + data.len());
    plaintext.extend_from_slice(&(pb.len() as u32).to_be_bytes());
    plaintext.extend_from_slice(pb);
    plaintext.extend_from_slice(&(data.len() as u32).to_be_bytes());
    plaintext.extend_from_slice(data);
    let frame = crate::network::content_crypto::seal(
        state,
        crate::network::content_crypto::FrameKind::Crdt,
        &plaintext,
    )
    .await?;
    anyhow::ensure!(frame.len() <= MAX_FRAME_BYTES, "sync frame too large");
    w.write_all(&(frame.len() as u32).to_be_bytes()).await?;
    w.write_all(&frame).await?;
    w.flush().await?;
    Ok(())
}

async fn read_frame<R: AsyncReadExt + Unpin>(
    r: &mut R,
    state: &AppState,
) -> Result<(String, Vec<u8>)> {
    let mut u32buf = [0u8; 4];
    r.read_exact(&mut u32buf).await?;
    let frame_len = u32::from_be_bytes(u32buf) as usize;
    anyhow::ensure!(
        frame_len <= MAX_FRAME_BYTES,
        "sync frame too large: {frame_len}"
    );
    let mut frame = vec![0; frame_len];
    r.read_exact(&mut frame).await?;
    let plaintext = crate::network::content_crypto::open(
        state,
        crate::network::content_crypto::FrameKind::Crdt,
        &frame,
    )
    .await?;
    let mut cursor = std::io::Cursor::new(plaintext);
    std::io::Read::read_exact(&mut cursor, &mut u32buf)?;
    let plen = u32::from_be_bytes(u32buf) as usize;
    anyhow::ensure!(plen <= MAX_FRAME_BYTES, "sync path too large");
    let mut pbuf = vec![0; plen];
    std::io::Read::read_exact(&mut cursor, &mut pbuf)?;
    let path = String::from_utf8(pbuf)?;
    std::io::Read::read_exact(&mut cursor, &mut u32buf)?;
    let dlen = u32::from_be_bytes(u32buf) as usize;
    anyhow::ensure!(dlen <= MAX_FRAME_BYTES, "sync payload too large");
    let mut data = vec![0; dlen];
    std::io::Read::read_exact(&mut cursor, &mut data)?;
    anyhow::ensure!(
        cursor.position() as usize == cursor.get_ref().len(),
        "trailing sync frame bytes"
    );
    Ok((path, data))
}

async fn write_u32<W: AsyncWriteExt + Unpin>(w: &mut W, n: u32) -> Result<()> {
    w.write_all(&n.to_be_bytes()).await?;
    w.flush().await?;
    Ok(())
}

async fn write_awareness_frame<W: AsyncWriteExt + Unpin>(
    w: &mut W,
    state: &AppState,
    path: &str,
    data: &[u8],
) -> Result<()> {
    let path = format!("{AWARENESS_PATH_PREFIX}{path}");
    write_frame(w, state, &path, data).await
}

async fn flush_pending_awareness<W: AsyncWriteExt + Unpin>(
    w: &mut W,
    state: &AppState,
    rx: &mut tokio::sync::broadcast::Receiver<(String, Vec<u8>)>,
    peer_id: PeerId,
) -> Result<()> {
    loop {
        match rx.try_recv() {
            Ok((path, data)) => write_awareness_frame(w, state, &path, &data).await?,
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
    write_frame(w, state, SESSION_PATH, &hello).await
}

async fn read_session_hello<R: AsyncReadExt + Unpin>(
    r: &mut R,
    state: &AppState,
) -> Result<PeerSession> {
    let (path, data) = read_frame(r, state).await?;
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
    let txn = state.control.try_transact().ok()?;
    let map = txn.get_map(MEMBER_LIST_KEY)?;
    match map.get(&txn, &peer_str) {
        Some(Out::Any(Any::String(s))) => serde_json::from_str::<MemberEntry>(&s)
            .ok()
            .map(|e| e.device_label),
        _ => None,
    }
}

/// Outcome of one attempt to apply a peer update.
enum ApplyOutcome {
    Applied,
    /// The doc is locked by another writer — worth retrying.
    Busy,
    /// Malformed or unapplicable update — retrying cannot help.
    Fatal,
}

/// One attempt, kept synchronous so the transaction guard (which is not `Send`)
/// never spans an await point and `run_sync` stays spawnable.
fn try_apply_once(doc: &Doc, raw: &[u8], path: &str) -> ApplyOutcome {
    let update = match Update::decode_v1(raw) {
        Ok(update) => update,
        Err(e) => {
            warn!("[sync] decode_v1 for {path}: {e}");
            return ApplyOutcome::Fatal;
        }
    };
    // "p2p" origin so the observer skips forwarding this back to all_updates,
    // preventing the update from echoing to the peer that just sent it.
    let Ok(mut txn) = doc.try_transact_mut_with("p2p") else {
        return ApplyOutcome::Busy;
    };
    if let Err(e) = txn.apply_update(update) {
        warn!("[sync] apply_update for {path}: {e}");
        return ApplyOutcome::Fatal;
    }
    ApplyOutcome::Applied
}

/// Apply an update received from a peer, retrying while the doc is busy.
///
/// Returning false tears the sync down and forces a reconnect that re-sends
/// everything from scratch, so a doc that is merely locked for a moment must
/// not be allowed to cause one.
async fn apply_update(state: &AppState, path: &str, raw: &[u8], peer_id: PeerId) -> bool {
    let doc = if path == "__control__" {
        state.control.clone()
    } else {
        state.get_or_create_doc(path)
    };

    let mut applied = false;
    for attempt in 0..CONTENTION_RETRIES {
        match try_apply_once(&doc, raw, path) {
            ApplyOutcome::Applied => {
                applied = true;
                break;
            }
            ApplyOutcome::Fatal => return false,
            ApplyOutcome::Busy => {
                tokio::time::sleep(CONTENTION_BACKOFF * (attempt + 1)).await;
            }
        }
    }
    if !applied {
        warn!("[sync] {path}: state busy after retries; deferring update to the next sync");
        return false;
    }

    if path != "__control__" {
        let author = device_label_for_peer(state, &peer_id);
        let state = state.clone();
        let path = path.to_string();
        tokio::spawn(async move {
            crate::store::fs::flush_to_disk(&state, &path, author).await;
        });
    }
    true
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
async fn full_state_update(state: &AppState, path: &str) -> Vec<u8> {
    let doc = if path == "__control__" {
        state.control.clone()
    } else {
        state.get_or_create_doc(path)
    };
    let full_diff = encode_diff_retry(&doc, &StateVector::default(), path).await;
    encode_sync(Message::Sync(SyncMessage::Update(full_diff)))
}

/// Whether a v1-encoded update carries no changes.
///
/// `encode_diff_v1` against an up-to-date state vector still produces a short
/// well-formed update — zero blocks and an empty delete set — rather than zero
/// bytes. Recognising it lets the catch-up push skip docs the peer already has,
/// which is the difference between a reconnect costing nothing and a reconnect
/// re-sending the entire workspace.
fn is_empty_update(diff: &[u8]) -> bool {
    Update::decode_v1(diff)
        .map(|update| update.state_vector().is_empty() && update.is_empty())
        .unwrap_or(false)
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

/// Revocation check for the sync loop.
///
/// Fails OPEN on lock contention. `try_is_peer_removed` returns `None` when the
/// control doc is momentarily locked by another writer — and the control doc is
/// the hottest doc in a circle, because presence heartbeats write to it
/// continuously. Reading that transient `None` as "revoked" tears down healthy
/// syncs against peers that were never removed. Only a real tombstone entry may
/// revoke; absence of an answer may not.
fn sync_revoked(state: &AppState, peer_id: &PeerId) -> bool {
    state.try_is_peer_removed(&state.peer_id).unwrap_or(false)
        || state
            .try_is_peer_removed(&peer_id.to_string())
            .unwrap_or(false)
}

fn mark_self_removed(state: &AppState) {
    use crate::control::{CircleEvent, MEMBER_LIST_KEY};

    let removed_at = chrono::Utc::now().to_rfc3339();
    {
        let mut txn = match state.control.try_transact_mut() {
            Ok(txn) => txn,
            Err(_) => {
                warn!("[sync] state busy while recording local revocation");
                return;
            }
        };
        use yrs::WriteTxn;
        let removed = txn.get_or_insert_map(MLS_REMOVED_KEY);
        let members = txn.get_or_insert_map(MEMBER_LIST_KEY);
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
        // Deliberately `warn`, not `debug`. Every reason a sync ends early —
        // circle mismatch, revocation, lock contention, a torn stream — used to
        // land below the daemon's default level, so a circle that had silently
        // stopped syncing looked identical in the log to one that was healthy.
        warn!("[sync] {peer_id}: sync ended: {e}");
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
        peer_session = read_session_hello(&mut rx, state).await?;
    } else {
        peer_session = read_session_hello(&mut rx, state).await?;
        write_session_hello(&mut tx, state, remote_is_removed).await?;
    }

    if peer_session.you_are_removed {
        mark_self_removed(state);
        warn!(
            "[sync] this device was removed from circle {}",
            state.circle_id
        );
        return Err(anyhow::anyhow!("this device was removed from circle"));
    }
    if remote_is_removed || state.is_self_removed() {
        warn!("[sync] rejected {peer_id}: explicitly removed from this circle");
        return Err(anyhow::anyhow!("peer rejected: removed from circle"));
    }

    if let Some(remote_circle_id) = &peer_session.circle_id {
        if remote_circle_id != &state.circle_id {
            // Remember it, so the swarm stops dialing this peer for this circle.
            // Without this the connection is torn down here and immediately
            // re-established from the same Kademlia routing entry.
            if state.mark_foreign_peer(&peer_id.to_string()) {
                warn!(
                    "[sync] {peer_id} belongs to circle {remote_circle_id}; suppressing further dials from circle {}",
                    state.circle_id
                );
            }
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

    // A doc that is busy right now is skipped, not fatal. This map only feeds
    // conflict detection, and both readers below already treat a missing entry
    // as "no pre-merge baseline" and move on. Aborting the whole sync here
    // instead means one momentarily-locked file blocks every other file in the
    // circle from syncing at all.
    let mut pre_merge: HashMap<String, (Vec<u8>, String)> = HashMap::new();
    let mut skipped_snapshots = 0usize;
    for entry in state.docs.iter() {
        let path = entry.key().clone();
        let Ok(txn) = entry.value().try_transact() else {
            skipped_snapshots += 1;
            continue;
        };
        let sv = txn.state_vector().encode_v1();
        let content = txn
            .get_text(path.as_str())
            .map(|text| text.get_string(&txn))
            .unwrap_or_default();
        pre_merge.insert(path, (sv, content));
    }
    if skipped_snapshots > 0 {
        debug!("[sync] {skipped_snapshots} doc(s) busy during pre-merge snapshot; conflict detection skipped for those");
    }

    // State vectors the peer reports during the handshake, keyed by doc path.
    // Absent = the peer never mentioned that doc, so it does not have it.
    let mut peer_svs: HashMap<String, StateVector> = HashMap::new();

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
            let sv = state_vector_retry(&doc, path).await;
            write_frame(
                &mut tx,
                state,
                path,
                &encode_sync(Message::Sync(SyncMessage::SyncStep1(sv))),
            )
            .await?;
        }

        // Read SyncStep2 replies (one per our SyncStep1)
        for _ in 0..my_paths.len() {
            let (path, data) = read_frame(&mut rx, state).await?;
            if let IncomingEvent::Apply { raw_update, .. } = parse_frame(path.clone(), &data) {
                if !apply_update(state, &path, &raw_update, peer_id).await {
                    return Err(anyhow::anyhow!("circle state busy; reconnecting sync"));
                }
            }
        }

        // Read responder's SyncStep1 count + messages, send SyncStep2 for each.
        // SyncStep2 was already applied above, so we use the pre-merge snapshot
        // for divergence detection rather than the (already merged) current state.
        let their_count = read_u32(&mut rx).await? as usize;
        for _ in 0..their_count {
            let (path, data) = read_frame(&mut rx, state).await?;
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
                let diff = encode_diff_retry(&doc, &sv, &path).await;
                write_frame(
                    &mut tx,
                    state,
                    &path,
                    &encode_sync(Message::Sync(SyncMessage::SyncStep2(diff))),
                )
                .await?;
                // Remember what the peer already has. The catch-up push below
                // uses it to send a diff instead of the entire doc history.
                peer_svs.insert(path, sv);
            }
        }
    } else {
        // Read initiator's SyncStep1 messages, send SyncStep2 for each
        let their_count = read_u32(&mut rx).await? as usize;
        for _ in 0..their_count {
            let (path, data) = read_frame(&mut rx, state).await?;
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
                let diff = encode_diff_retry(&doc, &sv, &path).await;
                write_frame(
                    &mut tx,
                    state,
                    &path,
                    &encode_sync(Message::Sync(SyncMessage::SyncStep2(diff))),
                )
                .await?;
                // Remember what the peer already has. The catch-up push below
                // uses it to send a diff instead of the entire doc history.
                peer_svs.insert(path, sv);
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
            let sv = state_vector_retry(&doc, path).await;
            write_frame(
                &mut tx,
                state,
                path,
                &encode_sync(Message::Sync(SyncMessage::SyncStep1(sv))),
            )
            .await?;
        }

        // Read initiator's SyncStep2 replies
        for _ in 0..my_paths.len() {
            let (path, data) = read_frame(&mut rx, state).await?;
            if let IncomingEvent::Apply { raw_update, .. } = parse_frame(path.clone(), &data) {
                if !apply_update(state, &path, &raw_update, peer_id).await {
                    return Err(anyhow::anyhow!("circle state busy; reconnecting sync"));
                }
            }
        }
    }

    tracing::info!("[sync] handshake complete with {peer_id}");

    if state.is_peer_removed(&peer_id.to_string()) {
        let _ = write_frame(&mut tx, state, REVOKED_PATH, &[]).await;
        return Err(anyhow::anyhow!("peer removed during sync handshake"));
    }
    if state.is_self_removed() {
        return Err(anyhow::anyhow!(
            "this device was removed during sync handshake"
        ));
    }

    // Subscribe to awareness before the handshake, then flush anything that
    // happened while CRDT/session/conflict setup was running. Otherwise cursor
    // frames produced during the handshake are lost because awareness is
    // ephemeral and broadcast receivers only see events after subscription.
    flush_pending_awareness(&mut tx, state, &mut all_awareness_rx, peer_id).await?;

    // Subscribe to local updates BEFORE the catch-up push, not after it.
    //
    // The catch-up sends what the peer is missing as of now; the broadcast
    // carries everything produced from now on. Subscribing afterwards left a
    // gap between the two, and anything written in it reached the peer neither
    // way — it was not in the push, and nobody was listening yet. Chat lives in
    // the control doc, which the push sends first, so a message posted while
    // the rest of the workspace was still being pushed fell into that gap and
    // stayed missing until the next reconnect. That is the "first few messages
    // don't sync" case.
    //
    // Subscribing first makes the two overlap instead of leaving a gap. An
    // update caught by both is applied twice, which is free: CRDT updates are
    // idempotent, which is the same reason the lag path below can re-send
    // everything.
    let mut all_rx = state.all_updates.subscribe();
    let mut all_deletes_rx = state.all_deletes.subscribe();

    // ── Post-handshake catch-up ───────────────────────────────────────────────
    //
    // The handshake only covers docs that were open on BOTH sides at the moment
    // it ran. If one side has docs the other doesn't know about, those are missed.
    // Fix: immediately push our full CRDT state for every doc we hold as Update
    // messages — the peer applies them idempotently via the continuous exchange
    // reader. Both sides do this, so convergence is guaranteed regardless of the
    // initial asymmetry.
    // Revocation is checked once here, not once per doc. In a circle with
    // hundreds of files this loop runs hundreds of times, and re-reading the
    // control doc on every iteration gave a transient lock hundreds of chances
    // to abort an otherwise healthy catch-up. The continuous exchange below
    // re-checks revocation on every frame, so a peer removed mid-catch-up is
    // still cut off promptly.
    if sync_revoked(state, &peer_id) {
        if state.is_peer_removed(&peer_id.to_string()) {
            let _ = write_frame(&mut tx, state, REVOKED_PATH, &[]).await;
        }
        return Err(anyhow::anyhow!("peer removed during sync catch-up"));
    }
    let mut pushed = 0usize;
    let mut skipped = 0usize;
    for path in all_doc_paths(state) {
        let msg = match peer_svs.get(&path) {
            // The peer told us what it has, so send only what it is missing.
            // For an already-converged doc that is an empty update, and sending
            // it would be pure head-of-line blocking in front of live edits.
            Some(their_sv) => {
                let doc = if path == "__control__" {
                    state.control.clone()
                } else {
                    state.get_or_create_doc(&path)
                };
                let diff = encode_diff_retry(&doc, their_sv, &path).await;
                if is_empty_update(&diff) {
                    skipped += 1;
                    continue;
                }
                encode_sync(Message::Sync(SyncMessage::Update(diff)))
            }
            // Never mentioned in the handshake — the peer does not have this
            // doc at all, so it needs the whole thing.
            None => full_state_update(state, &path).await,
        };
        pushed += 1;
        write_frame(&mut tx, state, &path, &msg).await?;
    }
    info!(
        "[sync] catch-up to {peer_id}: pushed {pushed} doc(s), skipped {skipped} already in sync"
    );
    flush_pending_awareness(&mut tx, state, &mut all_awareness_rx, peer_id).await?;

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
    let reader_state = state.clone();

    tokio::spawn(async move {
        loop {
            match read_frame(&mut rx, &reader_state).await {
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
                        let _ = write_frame(&mut tx, state, REVOKED_PATH, &[]).await;
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
                        if !apply_update(state, &path, &raw_update, peer_id).await {
                            return Err(anyhow::anyhow!("circle state busy; reconnecting sync"));
                        }
                    }
                    IncomingEvent::ResyncRequest { path, sv } => {
                        // Peer lagged — send them our full state for this doc
                        let sv = StateVector::decode_v1(&sv)?;
                        let doc = if path == "__control__" { state.control.clone() } else { state.get_or_create_doc(&path) };
                        let diff = encode_diff_retry(&doc, &sv, &path).await;
                        let step2 = encode_sync(Message::Sync(SyncMessage::SyncStep2(diff)));
                        write_frame(&mut tx, state, &path, &step2).await?;
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
                        let _ = write_frame(&mut tx, state, REVOKED_PATH, &[]).await;
                    }
                    warn!("[sync] stopped forwarding updates to removed peer {peer_id}");
                    break;
                }
                match result {
                    Ok((path, raw)) => {
                        let msg = encode_sync(Message::Sync(SyncMessage::Update(raw)));
                        write_frame(&mut tx, state, &path, &msg).await?;
                    }
                    Err(RecvError::Lagged(n)) => {
                        // We dropped n updates. Send our full CRDT state for every
                        // doc so the peer is guaranteed to converge. CRDT merges
                        // are idempotent so re-sending existing state is safe.
                        warn!("[sync] lagged {n} updates to {peer_id} — sending full state");
                        for path in all_doc_paths(state) {
                            let msg = full_state_update(state, &path).await;
                            write_frame(&mut tx, state, &path, &msg).await?;
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
                        write_awareness_frame(&mut tx, state, &path, &data).await?;
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
                        write_frame(&mut tx, state, &path, &[]).await?;
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

    fn test_state() -> AppState {
        AppState::new(
            "circle-local".into(),
            "Circle".into(),
            std::path::PathBuf::new(),
            std::path::PathBuf::new(),
            String::new(),
            "agent".into(),
            1,
            "peer-local".into(),
            crate::config::JoinPolicy::Manual,
            "owner".into(),
            crate::mls::new_mls_state(
                crate::mls::MlsIdentity::generate("peer-local").unwrap(),
                None,
            ),
        )
    }

    /// Regression: a momentarily locked control doc must not read as
    /// revocation. `try_is_peer_removed` returns `None` under contention, and
    /// defaulting that to "removed" tore down healthy syncs — the control doc
    /// is written constantly by presence heartbeats, and the catch-up loop
    /// consulted it once per doc, so large circles aborted almost every pass.
    #[test]
    fn contended_control_doc_does_not_read_as_revocation() {
        let state = test_state();
        let peer = PeerId::random();

        // Hold a write transaction, exactly as a concurrent writer would.
        let _held = state.control.try_transact_mut().unwrap();

        assert!(
            state.try_is_peer_removed(&peer.to_string()).is_none(),
            "precondition: the contended doc should refuse a read transaction"
        );
        assert!(
            !sync_revoked(&state, &peer),
            "lock contention must not be reported as revocation"
        );
    }

    #[test]
    fn revocation_is_still_detected_when_the_doc_is_readable() {
        use yrs::WriteTxn;
        let state = test_state();
        let peer = PeerId::random();

        {
            let mut txn = state.control.try_transact_mut().unwrap();
            let removed = txn.get_or_insert_map(MLS_REMOVED_KEY);
            removed.insert(&mut txn, peer.to_string().as_str(), "2026-08-27T00:00:00Z");
        }

        assert!(sync_revoked(&state, &peer), "a real tombstone must revoke");
    }

    /// A confirmed cross-circle peer is recorded once so the swarm can stop
    /// redialing it; peers we know nothing about stay dialable, because a fresh
    /// joiner is indistinguishable from an unknown peer until it syncs.
    #[test]
    fn foreign_peers_are_recorded_once_and_unknown_peers_stay_dialable() {
        let state = test_state();
        let foreign = PeerId::random().to_string();
        let unknown = PeerId::random().to_string();

        assert!(state.mark_foreign_peer(&foreign), "first mark is new");
        assert!(
            !state.mark_foreign_peer(&foreign),
            "second mark is a repeat"
        );

        assert!(state.is_foreign_peer(&foreign));
        assert!(!state.is_foreign_peer(&unknown));
    }

    /// `is_empty_update` gates whether the catch-up push skips a doc, so a
    /// false positive would silently drop real data. Pin both directions.
    #[test]
    fn empty_update_detection_distinguishes_converged_from_pending() {
        use yrs::{Text, Transact, WriteTxn};

        let doc = Doc::new();
        {
            let mut txn = doc.transact_mut();
            let text = txn.get_or_insert_text("f");
            text.push(&mut txn, "hello");
        }

        // A diff against a peer that already has everything carries no changes.
        let converged_sv = doc.transact().state_vector();
        let converged = doc.transact().encode_diff_v1(&converged_sv);
        assert!(
            is_empty_update(&converged),
            "diff against an up-to-date state vector must read as empty"
        );

        // A diff against a peer that has nothing must NOT read as empty.
        let full = doc.transact().encode_diff_v1(&StateVector::default());
        assert!(
            !is_empty_update(&full),
            "full state of a non-empty doc must never be skipped"
        );

        // A diff carrying only the newest edit must NOT read as empty.
        {
            let mut txn = doc.transact_mut();
            let text = txn.get_or_insert_text("f");
            text.push(&mut txn, " world");
        }
        let incremental = doc.transact().encode_diff_v1(&converged_sv);
        assert!(
            !is_empty_update(&incremental),
            "a diff containing a real edit must never be skipped"
        );
    }

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
