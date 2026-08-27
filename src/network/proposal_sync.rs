//! Encrypted proposal pull protocol — `/enoxian/proposals/2.0.0`.
//!
//! Proposals are durable, ever-growing review history. Replicating them through
//! the in-memory, fully-replicated control doc made it grow without bound (see
//! `docs/reference/p2p-protocols.md`). Instead, on each peer connection both
//! sides run a one-shot anti-entropy exchange against their on-disk proposal
//! stores and transfer only what the other lacks:
//!
//! ```text
//! 1. both send HAVE { (id, fingerprint) for every local proposal }
//! 2. each computes which ids it wants (missing, or fingerprint differs)
//! 3. each sends WANT { ids }
//! 4. each streams BUNDLE { ProposalBundle } for every id the peer wanted
//! 5. received bundles are applied via ProposalBundle::apply_to_store, whose
//!    status conflict rule decides whether an inbound record wins
//! 6. each side requests any content-addressed blobs referenced by local
//!    proposal manifests but missing from its blob store
//! ```
//!
//! Runs once per connection (no timer, no eager push). The disk store is the
//! source of truth; `ProposalBundle` is the transfer unit, reused unchanged.

use anyhow::{Context, Result};
use libp2p::{PeerId, Stream, StreamProtocol};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::broadcast;
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tracing::{debug, warn};

use crate::proposal::store::ProposalStore;
use crate::proposal::sync::ProposalBundle;
use crate::state::AppState;

pub const PROTOCOL: StreamProtocol = StreamProtocol::new("/enoxian/proposals/2.0.0");
const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

/// One advertised proposal: its id and a fingerprint of its mutable state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Have {
    pub id: String,
    pub fingerprint: u64,
}

/// The three message kinds, length-prefixed JSON on the wire.
#[derive(Debug, Serialize, Deserialize)]
enum Msg {
    Have(Vec<Have>),
    Want(Vec<String>),
    Bundles(Vec<ProposalBundle>),
    WantBlobs(Vec<String>),
    Blobs(Vec<BlobPayload>),
}

/// A content-addressed blob transferred after proposal manifests are present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobPayload {
    pub hash: String,
    /// Base64 keeps the length-prefixed JSON frame binary-safe.
    pub bytes_b64: String,
}

// ── Pure delta computation (unit-tested) ─────────────────────────────────────

/// Given what the local store holds and what the peer advertised, return the
/// ids the local side should request: every id the peer has that we either lack
/// or hold with a different fingerprint (a status divergence). Whether a fetched
/// record actually replaces the local one is decided later by the conflict rule
/// in `apply_to_store` — here we only decide what is worth fetching.
pub fn compute_wants(local: &[Have], peer: &[Have]) -> Vec<String> {
    let local_by_id: BTreeMap<&str, u64> = local
        .iter()
        .map(|h| (h.id.as_str(), h.fingerprint))
        .collect();
    peer.iter()
        .filter(|p| local_by_id.get(p.id.as_str()) != Some(&p.fingerprint))
        .map(|p| p.id.clone())
        .collect()
}

// ── Store helpers ────────────────────────────────────────────────────────────

fn local_haves(store: &ProposalStore) -> Vec<Have> {
    store
        .list_proposals()
        .into_iter()
        .map(|p| Have {
            id: p.id.clone(),
            fingerprint: p.fingerprint(),
        })
        .collect()
}

fn bundles_for(store: &ProposalStore, ids: &[String]) -> Vec<ProposalBundle> {
    ids.iter()
        .filter_map(|id| store.load_proposal(id).ok())
        .filter_map(|p| ProposalBundle::from_store(store, &p).ok())
        .collect()
}

fn proposal_blob_hashes(store: &ProposalStore) -> BTreeSet<String> {
    let mut hashes = BTreeSet::new();
    for proposal in store.list_proposals() {
        let Ok(base) = store.load_snapshot(&proposal.base_snapshot) else {
            continue;
        };
        let Ok(result) = store.load_snapshot(&proposal.result_snapshot) else {
            continue;
        };
        for path in &proposal.changed_paths {
            if let Some(entry) = base.files.get(path) {
                hashes.insert(entry.hash.clone());
            }
            if let Some(entry) = result.files.get(path) {
                hashes.insert(entry.hash.clone());
            }
        }
    }
    hashes
}

pub fn missing_blob_hashes(store: &ProposalStore) -> Vec<String> {
    proposal_blob_hashes(store)
        .into_iter()
        .filter(|hash| !store.blobs.contains(hash))
        .collect()
}

fn blob_payloads_for(store: &ProposalStore, hashes: &[String]) -> Vec<BlobPayload> {
    hashes
        .iter()
        .filter_map(|hash| {
            let bytes = store.blobs.get(hash).ok()?;
            // Verify before serving so the content-addressed invariant is
            // preserved even if the local store was corrupted out of band.
            if crate::proposal::blob::BlobStore::hash(&bytes) != *hash {
                return None;
            }
            Some(BlobPayload {
                hash: hash.clone(),
                bytes_b64: base64_encode(&bytes),
            })
        })
        .collect()
}

fn apply_blob_payloads(store: &ProposalStore, blobs: &[BlobPayload]) -> Result<usize> {
    let mut applied = 0usize;
    for blob in blobs {
        let bytes = match base64_decode(&blob.bytes_b64) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };
        if crate::proposal::blob::BlobStore::hash(&bytes) != blob.hash {
            continue;
        }
        if !store.blobs.contains(&blob.hash) {
            store.blobs.put(&bytes)?;
            applied += 1;
        }
    }
    Ok(applied)
}

// ── Wire framing: [u32 len][JSON] ────────────────────────────────────────────

async fn write_msg<W: AsyncWriteExt + Unpin>(w: &mut W, state: &AppState, msg: &Msg) -> Result<()> {
    let bytes = serde_json::to_vec(msg)?;
    let frame = crate::network::content_crypto::seal(
        state,
        crate::network::content_crypto::FrameKind::Proposal,
        &bytes,
    )
    .await?;
    anyhow::ensure!(frame.len() <= MAX_FRAME_BYTES, "proposal frame too large");
    w.write_all(&(frame.len() as u32).to_be_bytes()).await?;
    w.write_all(&frame).await?;
    w.flush().await?;
    Ok(())
}

async fn read_msg<R: AsyncReadExt + Unpin>(r: &mut R, state: &AppState) -> Result<Msg> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    // Guard against a malformed/hostile length prefix.
    anyhow::ensure!(len <= MAX_FRAME_BYTES, "proposal frame too large: {len}");
    let mut frame = vec![0u8; len];
    r.read_exact(&mut frame).await?;
    let bytes = crate::network::content_crypto::open(
        state,
        crate::network::content_crypto::FrameKind::Proposal,
        &frame,
    )
    .await?;
    serde_json::from_slice(&bytes).context("decoding proposal message")
}

// ── Entry points (mirror sync::run_sync) ─────────────────────────────────────

pub async fn run(peer_id: PeerId, stream: Stream, state: AppState, is_initiator: bool) {
    let mut events = state.events.subscribe();
    let sync_peer = peer_id;
    let result = tokio::select! {
        result = run_inner(sync_peer, stream, &state, is_initiator) => result,
        _ = wait_for_revocation(&state, &peer_id, &mut events) => {
            Err(anyhow::anyhow!("peer removed during proposal sync"))
        }
    };
    if let Err(e) = result {
        debug!("[proposal-sync] {peer_id}: {e}");
    }
}

async fn wait_for_revocation(
    state: &AppState,
    peer_id: &PeerId,
    events: &mut broadcast::Receiver<crate::control::CircleEvent>,
) {
    let peer_id = peer_id.to_string();
    loop {
        if state.is_self_removed() || state.is_peer_removed(&peer_id) {
            return;
        }
        match events.recv().await {
            Ok(crate::control::CircleEvent::MemberRemoved { peer_id: removed })
                if removed == peer_id || removed == state.peer_id =>
            {
                return;
            }
            Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

fn ensure_peer_authorized(state: &AppState, peer_id: &PeerId) -> Result<()> {
    anyhow::ensure!(
        !state.is_self_removed() && !state.is_peer_removed(&peer_id.to_string()),
        "peer removed from circle"
    );
    anyhow::ensure!(
        !state.is_foreign_peer(&peer_id.to_string()),
        "peer belongs to another circle"
    );
    Ok(())
}

async fn run_inner(
    peer_id: PeerId,
    stream: Stream,
    state: &AppState,
    is_initiator: bool,
) -> Result<()> {
    // Rechecked between every exchange phase so an in-flight sync cannot keep
    // transferring proposal manifests or blobs after membership is revoked.
    ensure_peer_authorized(state, &peer_id)?;

    let store = ProposalStore::open(&state.workspace)?;
    let compat = stream.compat();
    let (mut rx, mut tx) = tokio::io::split(compat);

    // Exchange HAVE. Initiator writes first, responder reads first — symmetric,
    // deadlock-free (same ordering discipline as the CRDT sync handshake).
    let my_haves = local_haves(&store);
    let peer_haves = if is_initiator {
        ensure_peer_authorized(state, &peer_id)?;
        write_msg(&mut tx, state, &Msg::Have(my_haves)).await?;
        match read_msg(&mut rx, state).await? {
            Msg::Have(h) => h,
            _ => return Err(anyhow::anyhow!("expected HAVE")),
        }
    } else {
        let h = match read_msg(&mut rx, state).await? {
            Msg::Have(h) => h,
            _ => return Err(anyhow::anyhow!("expected HAVE")),
        };
        ensure_peer_authorized(state, &peer_id)?;
        write_msg(&mut tx, state, &Msg::Have(local_haves(&store))).await?;
        h
    };
    ensure_peer_authorized(state, &peer_id)?;

    // Decide what each side wants and exchange WANT, same ordering.
    let want = compute_wants(&local_haves(&store), &peer_haves);
    let peer_want = if is_initiator {
        ensure_peer_authorized(state, &peer_id)?;
        write_msg(&mut tx, state, &Msg::Want(want.clone())).await?;
        match read_msg(&mut rx, state).await? {
            Msg::Want(w) => w,
            _ => return Err(anyhow::anyhow!("expected WANT")),
        }
    } else {
        let w = match read_msg(&mut rx, state).await? {
            Msg::Want(w) => w,
            _ => return Err(anyhow::anyhow!("expected WANT")),
        };
        ensure_peer_authorized(state, &peer_id)?;
        write_msg(&mut tx, state, &Msg::Want(want.clone())).await?;
        w
    };
    ensure_peer_authorized(state, &peer_id)?;

    // Send the bundles the peer asked for; receive the ones we asked for.
    let outgoing = bundles_for(&store, &peer_want);
    let incoming = if is_initiator {
        ensure_peer_authorized(state, &peer_id)?;
        write_msg(&mut tx, state, &Msg::Bundles(outgoing)).await?;
        match read_msg(&mut rx, state).await? {
            Msg::Bundles(b) => b,
            _ => return Err(anyhow::anyhow!("expected BUNDLES")),
        }
    } else {
        let b = match read_msg(&mut rx, state).await? {
            Msg::Bundles(b) => b,
            _ => return Err(anyhow::anyhow!("expected BUNDLES")),
        };
        ensure_peer_authorized(state, &peer_id)?;
        write_msg(&mut tx, state, &Msg::Bundles(outgoing)).await?;
        b
    };
    ensure_peer_authorized(state, &peer_id)?;

    let mut applied = 0usize;
    for bundle in &incoming {
        if let Err(e) = crate::proposal::validate_bundle_for_circle(bundle, &state.circle_id) {
            warn!("[proposal-sync] rejected {}: {e}", bundle.proposal.id);
            continue;
        }
        match bundle.apply_to_store(&store) {
            Ok(true) => {
                applied += 1;
                let status = serde_json::to_value(bundle.status())
                    .ok()
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_default();
                let _ = state
                    .events
                    .send(crate::control::CircleEvent::ProposalUpdated {
                        proposal_id: bundle.proposal.id.clone(),
                        status,
                    });
            }
            Ok(false) => {}
            Err(e) => warn!("[proposal-sync] applying {}: {e}", bundle.proposal.id),
        }
    }
    // M15 events are authoritative for proposal decisions. If the proposal
    // bundle and event streams raced, reconcile now that metadata is present.
    if let Ok(events) =
        crate::workspace_event::EventStore::open(&state.workspace, state.circle_id.clone())
    {
        crate::network::event_sync::reconcile_proposals(state, &events, &store);
    }

    // After manifests arrive, fetch the missing content-addressed blobs they
    // reference. This is what lets oversized proposal files become reviewable
    // and revertible on peers that did not originate the change.
    let want_blobs = missing_blob_hashes(&store);
    let peer_want_blobs = if is_initiator {
        ensure_peer_authorized(state, &peer_id)?;
        write_msg(&mut tx, state, &Msg::WantBlobs(want_blobs.clone())).await?;
        match read_msg(&mut rx, state).await? {
            Msg::WantBlobs(w) => w,
            _ => return Err(anyhow::anyhow!("expected WANT_BLOBS")),
        }
    } else {
        let w = match read_msg(&mut rx, state).await? {
            Msg::WantBlobs(w) => w,
            _ => return Err(anyhow::anyhow!("expected WANT_BLOBS")),
        };
        ensure_peer_authorized(state, &peer_id)?;
        write_msg(&mut tx, state, &Msg::WantBlobs(want_blobs.clone())).await?;
        w
    };
    ensure_peer_authorized(state, &peer_id)?;

    let outgoing_blobs = blob_payloads_for(&store, &peer_want_blobs);
    let incoming_blobs = if is_initiator {
        ensure_peer_authorized(state, &peer_id)?;
        write_msg(&mut tx, state, &Msg::Blobs(outgoing_blobs)).await?;
        match read_msg(&mut rx, state).await? {
            Msg::Blobs(b) => b,
            _ => return Err(anyhow::anyhow!("expected BLOBS")),
        }
    } else {
        let b = match read_msg(&mut rx, state).await? {
            Msg::Blobs(b) => b,
            _ => return Err(anyhow::anyhow!("expected BLOBS")),
        };
        ensure_peer_authorized(state, &peer_id)?;
        write_msg(&mut tx, state, &Msg::Blobs(outgoing_blobs)).await?;
        b
    };

    ensure_peer_authorized(state, &peer_id)?;
    let applied_blobs = apply_blob_payloads(&store, &incoming_blobs)?;
    if applied > 0 || !peer_want.is_empty() {
        debug!(
            "[proposal-sync] {peer_id}: sent {}, applied {applied}, wanted blobs {}, applied blobs {applied_blobs}",
            peer_want.len(),
            want_blobs.len()
        );
    }
    Ok(())
}

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn base64_decode(s: &str) -> Result<Vec<u8>> {
    use base64::Engine;
    Ok(base64::engine::general_purpose::STANDARD.decode(s)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proposal::model::Proposal;
    use crate::proposal::snapshot::{FileEntry, Snapshot};
    use std::collections::BTreeMap;

    fn have(id: &str, fp: u64) -> Have {
        Have {
            id: id.into(),
            fingerprint: fp,
        }
    }

    #[test]
    fn wants_ids_we_lack() {
        let local = vec![have("a", 1)];
        let peer = vec![have("a", 1), have("b", 5)];
        assert_eq!(compute_wants(&local, &peer), vec!["b".to_string()]);
    }

    #[test]
    fn wants_ids_with_diverged_fingerprint() {
        // Same id, different fingerprint (peer changed status) -> refetch.
        let local = vec![have("a", 1)];
        let peer = vec![have("a", 2)];
        assert_eq!(compute_wants(&local, &peer), vec!["a".to_string()]);
    }

    #[test]
    fn wants_nothing_when_in_sync() {
        let local = vec![have("a", 1), have("b", 2)];
        let peer = vec![have("a", 1), have("b", 2)];
        assert!(compute_wants(&local, &peer).is_empty());
    }

    #[test]
    fn does_not_want_ids_only_we_have() {
        let local = vec![have("a", 1), have("local-only", 9)];
        let peer = vec![have("a", 1)];
        assert!(compute_wants(&local, &peer).is_empty());
    }

    fn snap_with_hash(path: &str, hash: String, size: u64) -> Snapshot {
        let mut files = BTreeMap::new();
        files.insert(path.to_string(), FileEntry { hash, size });
        Snapshot::new(files)
    }

    #[test]
    fn missing_blob_hashes_find_manifest_content_not_in_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProposalStore::open(dir.path()).unwrap();
        let present_hash = store.blobs.put(b"present").unwrap();
        let missing_hash = crate::proposal::blob::BlobStore::hash(b"missing");

        let base = snap_with_hash("big.bin", present_hash.clone(), 7);
        let result = snap_with_hash("big.bin", missing_hash.clone(), 7);
        store.save_snapshot(&base).unwrap();
        store.save_snapshot(&result).unwrap();
        let proposal = Proposal::ambient(
            "c".into(),
            base.id.clone(),
            result.id.clone(),
            vec!["big.bin".into()],
        );
        store.save_proposal(&proposal).unwrap();

        assert_eq!(missing_blob_hashes(&store), vec![missing_hash]);
    }

    #[test]
    fn blob_payloads_roundtrip_and_reject_bad_hashes() {
        let src_dir = tempfile::tempdir().unwrap();
        let src = ProposalStore::open(src_dir.path()).unwrap();
        let hash = src.blobs.put(b"large content").unwrap();

        let payloads = blob_payloads_for(&src, std::slice::from_ref(&hash));
        assert_eq!(payloads.len(), 1);

        let dst_dir = tempfile::tempdir().unwrap();
        let dst = ProposalStore::open(dst_dir.path()).unwrap();
        assert_eq!(apply_blob_payloads(&dst, &payloads).unwrap(), 1);
        assert_eq!(dst.blobs.get(&hash).unwrap(), b"large content");

        let bad = BlobPayload {
            hash,
            bytes_b64: base64_encode(b"different content"),
        };
        assert_eq!(apply_blob_payloads(&dst, &[bad]).unwrap(), 0);
    }
}
