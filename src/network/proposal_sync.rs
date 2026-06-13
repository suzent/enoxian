//! Proposal pull protocol — `/enoxian/proposals/1.0.0`.
//!
//! Proposals are durable, ever-growing review history. Replicating them through
//! the in-memory, fully-replicated control doc made it grow without bound (see
//! `docs/plan/proposal-pull-protocol.md`). Instead, on each peer connection both
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
//! ```
//!
//! Runs once per connection (no timer, no eager push). The disk store is the
//! source of truth; `ProposalBundle` is the transfer unit, reused unchanged.

use anyhow::{Context, Result};
use libp2p::{PeerId, Stream, StreamProtocol};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tracing::{debug, warn};

use crate::control::MLS_REMOVED_KEY;
use crate::proposal::store::ProposalStore;
use crate::proposal::sync::ProposalBundle;
use crate::state::AppState;

pub const PROTOCOL: StreamProtocol = StreamProtocol::new("/enoxian/proposals/1.0.0");

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
}

// ── Pure delta computation (unit-tested) ─────────────────────────────────────

/// Given what the local store holds and what the peer advertised, return the
/// ids the local side should request: every id the peer has that we either lack
/// or hold with a different fingerprint (a status divergence). Whether a fetched
/// record actually replaces the local one is decided later by the conflict rule
/// in `apply_to_store` — here we only decide what is worth fetching.
pub fn compute_wants(local: &[Have], peer: &[Have]) -> Vec<String> {
    let local_by_id: BTreeMap<&str, u64> =
        local.iter().map(|h| (h.id.as_str(), h.fingerprint)).collect();
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
        .map(|p| Have { id: p.id.clone(), fingerprint: p.fingerprint() })
        .collect()
}

fn bundles_for(store: &ProposalStore, ids: &[String]) -> Vec<ProposalBundle> {
    ids.iter()
        .filter_map(|id| store.load_proposal(id).ok())
        .filter_map(|p| ProposalBundle::from_store(store, &p).ok())
        .collect()
}

// ── Wire framing: [u32 len][JSON] ────────────────────────────────────────────

async fn write_msg<W: AsyncWriteExt + Unpin>(w: &mut W, msg: &Msg) -> Result<()> {
    let bytes = serde_json::to_vec(msg)?;
    w.write_all(&(bytes.len() as u32).to_be_bytes()).await?;
    w.write_all(&bytes).await?;
    w.flush().await?;
    Ok(())
}

async fn read_msg<R: AsyncReadExt + Unpin>(r: &mut R) -> Result<Msg> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    // Guard against a malformed/hostile length prefix.
    anyhow::ensure!(len <= 64 * 1024 * 1024, "proposal frame too large: {len}");
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    serde_json::from_slice(&buf).context("decoding proposal message")
}

// ── Entry points (mirror sync::run_sync) ─────────────────────────────────────

pub async fn run(peer_id: PeerId, stream: Stream, state: AppState, is_initiator: bool) {
    if let Err(e) = run_inner(peer_id, stream, &state, is_initiator).await {
        debug!("[proposal-sync] {peer_id}: {e}");
    }
}

async fn run_inner(
    peer_id: PeerId,
    stream: Stream,
    state: &AppState,
    is_initiator: bool,
) -> Result<()> {
    // Same membership gate as CRDT sync: an evicted peer must not pull history.
    {
        use yrs::{Map, Transact};
        let removed = state.control.get_or_insert_map(MLS_REMOVED_KEY);
        let txn = state.control.transact();
        if matches!(
            removed.get(&txn, peer_id.to_string().as_str()),
            Some(yrs::Out::Any(yrs::Any::String(_)))
        ) {
            warn!("[proposal-sync] rejected {peer_id}: removed from circle");
            return Err(anyhow::anyhow!("peer removed from circle"));
        }
    }

    let store = ProposalStore::open(&state.workspace)?;
    let compat = stream.compat();
    let (mut rx, mut tx) = tokio::io::split(compat);

    // Exchange HAVE. Initiator writes first, responder reads first — symmetric,
    // deadlock-free (same ordering discipline as the CRDT sync handshake).
    let my_haves = local_haves(&store);
    let peer_haves = if is_initiator {
        write_msg(&mut tx, &Msg::Have(my_haves)).await?;
        match read_msg(&mut rx).await? {
            Msg::Have(h) => h,
            _ => return Err(anyhow::anyhow!("expected HAVE")),
        }
    } else {
        let h = match read_msg(&mut rx).await? {
            Msg::Have(h) => h,
            _ => return Err(anyhow::anyhow!("expected HAVE")),
        };
        write_msg(&mut tx, &Msg::Have(local_haves(&store))).await?;
        h
    };

    // Decide what each side wants and exchange WANT, same ordering.
    let want = compute_wants(&local_haves(&store), &peer_haves);
    let peer_want = if is_initiator {
        write_msg(&mut tx, &Msg::Want(want.clone())).await?;
        match read_msg(&mut rx).await? {
            Msg::Want(w) => w,
            _ => return Err(anyhow::anyhow!("expected WANT")),
        }
    } else {
        let w = match read_msg(&mut rx).await? {
            Msg::Want(w) => w,
            _ => return Err(anyhow::anyhow!("expected WANT")),
        };
        write_msg(&mut tx, &Msg::Want(want.clone())).await?;
        w
    };

    // Send the bundles the peer asked for; receive the ones we asked for.
    let outgoing = bundles_for(&store, &peer_want);
    let incoming = if is_initiator {
        write_msg(&mut tx, &Msg::Bundles(outgoing)).await?;
        match read_msg(&mut rx).await? {
            Msg::Bundles(b) => b,
            _ => return Err(anyhow::anyhow!("expected BUNDLES")),
        }
    } else {
        let b = match read_msg(&mut rx).await? {
            Msg::Bundles(b) => b,
            _ => return Err(anyhow::anyhow!("expected BUNDLES")),
        };
        write_msg(&mut tx, &Msg::Bundles(outgoing)).await?;
        b
    };

    let mut applied = 0usize;
    for bundle in &incoming {
        match bundle.apply_to_store(&store) {
            Ok(true) => {
                applied += 1;
                let status = serde_json::to_value(bundle.status())
                    .ok()
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_default();
                let _ = state.events.send(crate::control::CircleEvent::ProposalUpdated {
                    proposal_id: bundle.proposal.id.clone(),
                    status,
                });
            }
            Ok(false) => {}
            Err(e) => warn!("[proposal-sync] applying {}: {e}", bundle.proposal.id),
        }
    }
    if applied > 0 || !peer_want.is_empty() {
        debug!(
            "[proposal-sync] {peer_id}: sent {}, applied {applied}",
            peer_want.len()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn have(id: &str, fp: u64) -> Have {
        Have { id: id.into(), fingerprint: fp }
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
}
