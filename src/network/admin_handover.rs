//! Admin key handover on leave — `/enoxian/admin-handover/1.0.0`.
//!
//! A circle's admin authority is one signing key, `admin.key`, whose public
//! half every member pins in its config. `enox leave` deletes the circle
//! directory, key included, and nothing can recreate it: a circle whose admin
//! left had no device able to approve anyone, ever again. So an admin that
//! leaves hands the key to a member first, while it still holds it.
//!
//! ```text
//! leaver → successor   OFFER  { circle_id, admin_key_hex }   (content-sealed)
//! successor → leaver   ACK    { ok, error }                  (content-sealed)
//! ```
//!
//! The successor keeps the key only if its public half is the admin key the
//! circle already pins, so a member cannot be talked into trusting a key it
//! did not already trust. The leaver deletes nothing until the ACK arrives.

use anyhow::{Context, Result};
use libp2p::{PeerId, Stream, StreamProtocol};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tracing::{info, warn};
use yrs::{Map, Out, ReadTxn, Transact, WriteTxn};

use crate::control::{MemberEntry, MemberRole, MEMBER_LIST_KEY, MLS_KEY_PACKAGES_KEY};
use crate::network::content_crypto::{self, FrameKind};
use crate::state::AppState;

pub const PROTOCOL: StreamProtocol = StreamProtocol::new("/enoxian/admin-handover/1.0.0");
const MAX_FRAME_BYTES: usize = 64 * 1024;
/// Covers opening the stream, both frames, and the successor writing the key.
const HANDOVER_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Serialize, Deserialize)]
struct Offer {
    circle_id: String,
    admin_key_hex: String,
}

#[derive(Serialize, Deserialize)]
struct Ack {
    ok: bool,
    error: Option<String>,
}

// ── Choosing a successor ─────────────────────────────────────────────────────

/// A member that could take the admin key over.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub peer_id: String,
    pub owner: String,
    pub added_at: chrono::DateTime<chrono::Utc>,
    /// Holds a current leaf in the MLS group. A member without one cannot
    /// commit, so it could hold the key but not approve anyone with it.
    pub admitted: bool,
    /// Connected right now. The key only travels over a live stream.
    pub connected: bool,
}

/// Pick who takes the admin key: `requested` if given, otherwise another
/// device of the leaving admin's own owner, otherwise the longest-standing
/// member. Only admitted, connected members qualify.
pub fn choose_successor(
    own_owner: &str,
    candidates: &[Candidate],
    requested: Option<&str>,
) -> Result<String, String> {
    if let Some(requested) = requested {
        let Some(c) = candidates.iter().find(|c| c.peer_id == requested) else {
            return Err(format!("{requested} is not a member of this circle"));
        };
        if !c.admitted {
            return Err(format!(
                "{requested} has not finished joining, so it could not approve anyone"
            ));
        }
        if !c.connected {
            return Err(format!("{requested} is not connected right now"));
        }
        return Ok(c.peer_id.clone());
    }
    candidates
        .iter()
        .filter(|c| c.admitted && c.connected)
        .min_by_key(|c| (c.owner != own_owner, c.added_at, c.peer_id.clone()))
        .map(|c| c.peer_id.clone())
        .ok_or_else(|| {
            "no other member is online to take over as admin — bring one of its \
             devices online and try again, or leave with --force to give admin \
             up for good"
                .to_string()
        })
}

/// Every other member of the circle, as a possible successor.
pub async fn candidates(state: &AppState) -> Result<Vec<Candidate>> {
    let entries: Vec<(MemberEntry, Option<Vec<u8>>)> = {
        let txn = state
            .control
            .try_transact()
            .map_err(|_| anyhow::anyhow!("circle state busy"))?;
        let packages = txn.get_map(MLS_KEY_PACKAGES_KEY);
        txn.get_map(MEMBER_LIST_KEY)
            .map(|members| {
                members
                    .iter(&txn)
                    .filter_map(|(_, value)| match value {
                        Out::Any(yrs::Any::String(s)) => {
                            serde_json::from_str::<MemberEntry>(&s).ok()
                        }
                        _ => None,
                    })
                    .map(|entry| {
                        let package = packages
                            .as_ref()
                            .and_then(|p| p.get(&txn, entry.peer_id.as_str()))
                            .and_then(|v| match v {
                                Out::Any(yrs::Any::String(s)) => hex::decode(s.as_ref()).ok(),
                                _ => None,
                            });
                        (entry, package)
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let mls = state.mls.lock().await;
    Ok(entries
        .into_iter()
        .filter(|(entry, _)| {
            entry.peer_id != state.peer_id && !state.is_peer_removed(&entry.peer_id)
        })
        .map(|(entry, package)| Candidate {
            admitted: mls.standing(&entry.peer_id, package.as_deref())
                == crate::mls::Standing::Current,
            connected: state.is_connected(&entry.peer_id),
            peer_id: entry.peer_id,
            owner: entry.owner,
            added_at: entry.added_at,
        })
        .collect())
}

// ── Leaver ───────────────────────────────────────────────────────────────────

/// Hand `admin_key_hex` to `successor` and wait until it has stored it.
pub async fn hand_over(state: &AppState, successor: &str, admin_key_hex: &str) -> Result<()> {
    let peer: PeerId = successor.parse().context("invalid successor peer id")?;
    let mut control = state
        .stream_control()
        .context("circle is not running, so the admin key cannot be handed over")?;
    tokio::time::timeout(HANDOVER_TIMEOUT, async {
        let stream = control
            .open_stream(peer, PROTOCOL)
            .await
            .map_err(|e| anyhow::anyhow!("could not reach {successor}: {e}"))?;
        offer(state, stream.compat(), admin_key_hex)
            .await
            .map_err(|e| anyhow::anyhow!("{successor}: {e}"))
    })
    .await
    .map_err(|_| anyhow::anyhow!("{successor} did not confirm the admin key in time"))?
}

async fn offer<S: AsyncRead + AsyncWrite>(
    state: &AppState,
    stream: S,
    admin_key_hex: &str,
) -> Result<()> {
    let (mut rx, mut tx) = tokio::io::split(stream);
    let offer = Offer {
        circle_id: state.circle_id.clone(),
        admin_key_hex: admin_key_hex.to_string(),
    };
    write_frame(&mut tx, state, &serde_json::to_vec(&offer)?).await?;
    let ack: Ack = serde_json::from_slice(&read_frame(&mut rx, state).await?)
        .context("invalid handover reply")?;
    if !ack.ok {
        anyhow::bail!("refused the admin key: {}", ack.error.unwrap_or_default());
    }
    Ok(())
}

// ── Successor ────────────────────────────────────────────────────────────────

pub async fn accept(peer: PeerId, stream: Stream, state: AppState) {
    serve(&state, &peer, stream.compat()).await;
}

async fn serve<S: AsyncRead + AsyncWrite>(state: &AppState, peer: &PeerId, stream: S) {
    let (mut rx, mut tx) = tokio::io::split(stream);
    let outcome = match read_frame(&mut rx, state).await {
        Ok(bytes) => receive(state, peer, &bytes).await,
        Err(e) => {
            warn!("[admin-handover] {peer}: {e}");
            return;
        }
    };
    let ack = match &outcome {
        Ok(()) => Ack {
            ok: true,
            error: None,
        },
        Err(e) => {
            warn!("[admin-handover] refused key from {peer}: {e}");
            Ack {
                ok: false,
                error: Some(e.to_string()),
            }
        }
    };
    let Ok(bytes) = serde_json::to_vec(&ack) else {
        return;
    };
    if let Err(e) = write_frame(&mut tx, state, &bytes).await {
        warn!("[admin-handover] {peer}: reply failed: {e}");
    }
}

async fn receive(state: &AppState, peer: &PeerId, bytes: &[u8]) -> Result<()> {
    let sender = peer.to_string();
    anyhow::ensure!(
        is_member(state, &sender) && !state.is_peer_removed(&sender),
        "sender is not a member of this circle"
    );
    let offer: Offer = serde_json::from_slice(bytes).context("invalid handover offer")?;
    anyhow::ensure!(
        offer.circle_id == state.circle_id,
        "offer is for another circle"
    );
    let keypair =
        crate::crypto::keypair_from_hex(offer.admin_key_hex.trim()).context("invalid admin key")?;
    anyhow::ensure!(
        hex::encode(keypair.public().encode_protobuf()) == state.admin_pubkey_hex.trim(),
        "the key is not this circle's admin key"
    );
    crate::config::write_secret(
        &state.circle_dir.join("admin.key"),
        offer.admin_key_hex.trim(),
    )
    .context("could not store the admin key")?;
    mark_self_admin(state);
    info!(
        "[admin-handover] now admin of circle {}, handed over by {sender}",
        state.circle_id
    );
    Ok(())
}

fn is_member(state: &AppState, peer_id: &str) -> bool {
    let Ok(txn) = state.control.try_transact() else {
        return false;
    };
    txn.get_map(MEMBER_LIST_KEY)
        .is_some_and(|members| members.get(&txn, peer_id).is_some())
}

/// Show the new role in the member list. Cosmetic — authority is the key file.
fn mark_self_admin(state: &AppState) {
    let Ok(mut txn) = state.control.try_transact_mut() else {
        return;
    };
    let members = txn.get_or_insert_map(MEMBER_LIST_KEY);
    let Some(mut entry) = members
        .get(&txn, state.peer_id.as_str())
        .and_then(|v| match v {
            Out::Any(yrs::Any::String(s)) => serde_json::from_str::<MemberEntry>(&s).ok(),
            _ => None,
        })
    else {
        return;
    };
    entry.role = MemberRole::Admin;
    if let Ok(json) = serde_json::to_string(&entry) {
        members.insert(&mut txn, state.peer_id.as_str(), json.as_str());
    }
}

// ── Framing ──────────────────────────────────────────────────────────────────

async fn write_frame<W: AsyncWriteExt + Unpin>(
    w: &mut W,
    state: &AppState,
    data: &[u8],
) -> Result<()> {
    let frame = content_crypto::seal(state, FrameKind::AdminHandover, data).await?;
    anyhow::ensure!(frame.len() <= MAX_FRAME_BYTES, "handover frame too large");
    w.write_all(&(frame.len() as u32).to_be_bytes()).await?;
    w.write_all(&frame).await?;
    w.flush().await?;
    Ok(())
}

async fn read_frame<R: AsyncReadExt + Unpin>(r: &mut R, state: &AppState) -> Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    anyhow::ensure!(len <= MAX_FRAME_BYTES, "handover frame too large: {len}");
    let mut frame = vec![0; len];
    r.read_exact(&mut frame).await?;
    content_crypto::open(state, FrameKind::AdminHandover, &frame).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mls::{MlsGroupManager, MlsIdentity};
    use chrono::{Duration as Days, Utc};
    use libp2p::identity::Keypair;
    use std::path::PathBuf;
    use yrs::WriteTxn;

    const CIRCLE: &str = "circle-under-test";

    struct Device {
        state: AppState,
        peer: PeerId,
        _dir: tempfile::TempDir,
    }

    fn device(admin_pubkey_hex: &str, mls: crate::mls::SharedMlsState, peer: PeerId) -> Device {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(
            CIRCLE.into(),
            "test".into(),
            PathBuf::from(dir.path()),
            PathBuf::from(dir.path()),
            admin_pubkey_hex.into(),
            "agent".into(),
            1,
            peer.to_string(),
            crate::config::JoinPolicy::Auto,
            "suzy".into(),
            mls,
        );
        Device {
            state,
            peer,
            _dir: dir,
        }
    }

    fn add_member(state: &AppState, peer: &PeerId) {
        let entry = MemberEntry {
            peer_id: peer.to_string(),
            owner: "suzy".into(),
            agent_id: "agent".into(),
            device_label: String::new(),
            agents: Vec::new(),
            ambient_agents: Vec::new(),
            role: MemberRole::Member,
            added_at: Utc::now(),
            signature: String::new(),
        };
        let mut txn = state.control.transact_mut();
        let members = txn.get_or_insert_map(MEMBER_LIST_KEY);
        members.insert(
            &mut txn,
            peer.to_string().as_str(),
            serde_json::to_string(&entry).unwrap().as_str(),
        );
    }

    /// An admin and a member in one MLS group, each knowing the other.
    fn circle() -> (Device, Device, Keypair) {
        let admin_key = Keypair::generate_ed25519();
        let pinned = hex::encode(admin_key.public().encode_protobuf());
        let leaver_peer = Keypair::generate_ed25519().public().to_peer_id();
        let successor_peer = Keypair::generate_ed25519().public().to_peer_id();

        let leaver_identity = MlsIdentity::generate(&leaver_peer.to_string()).unwrap();
        let successor_identity = MlsIdentity::generate(&successor_peer.to_string()).unwrap();
        let mut group = MlsGroupManager::create(&leaver_identity).unwrap();
        let (_, welcome, _) = group
            .add_member(
                &leaver_identity,
                &successor_identity.generate_key_package().unwrap(),
            )
            .unwrap();
        let joined =
            MlsGroupManager::join_from_welcome(&successor_identity, &welcome, None).unwrap();

        let leaver = device(
            &pinned,
            crate::mls::new_mls_state(leaver_identity, Some(group)),
            leaver_peer,
        );
        let successor = device(
            &pinned,
            crate::mls::new_mls_state(successor_identity, Some(joined)),
            successor_peer,
        );
        for d in [&leaver, &successor] {
            add_member(&d.state, &leaver.peer);
            add_member(&d.state, &successor.peer);
        }
        (leaver, successor, admin_key)
    }

    async fn run_handover(leaver: &Device, successor: &Device, key_hex: &str) -> Result<()> {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let serving = serve(&successor.state, &leaver.peer, b);
        let (offered, ()) = tokio::join!(offer(&leaver.state, a, key_hex), serving);
        offered
    }

    fn role_of(state: &AppState, peer: &PeerId) -> MemberRole {
        let txn = state.control.transact();
        let value = txn
            .get_map(MEMBER_LIST_KEY)
            .and_then(|m| m.get(&txn, peer.to_string().as_str()))
            .unwrap()
            .to_string(&txn);
        serde_json::from_str::<MemberEntry>(&value).unwrap().role
    }

    #[tokio::test]
    async fn the_successor_stores_the_pinned_admin_key_and_confirms() {
        let (leaver, successor, admin_key) = circle();
        let key_hex = crate::crypto::keypair_to_hex(&admin_key).unwrap();

        run_handover(&leaver, &successor, &key_hex).await.unwrap();

        let stored = std::fs::read_to_string(successor.state.circle_dir.join("admin.key")).unwrap();
        assert_eq!(stored, key_hex);
        assert_eq!(
            role_of(&successor.state, &successor.peer),
            MemberRole::Admin
        );
    }

    /// A member must not be talked into trusting a key the circle never pinned.
    #[tokio::test]
    async fn a_key_other_than_the_pinned_one_is_refused() {
        let (leaver, successor, _) = circle();
        let impostor = crate::crypto::keypair_to_hex(&Keypair::generate_ed25519()).unwrap();

        let error = run_handover(&leaver, &successor, &impostor)
            .await
            .unwrap_err();

        assert!(
            error.to_string().contains("not this circle's admin key"),
            "{error}"
        );
        assert!(!successor.state.circle_dir.join("admin.key").exists());
    }

    #[tokio::test]
    async fn a_key_from_a_non_member_is_refused() {
        let (leaver, successor, admin_key) = circle();
        {
            let mut txn = successor.state.control.transact_mut();
            let members = txn.get_or_insert_map(MEMBER_LIST_KEY);
            members.remove(&mut txn, leaver.peer.to_string().as_str());
        }
        let key_hex = crate::crypto::keypair_to_hex(&admin_key).unwrap();

        assert!(run_handover(&leaver, &successor, &key_hex).await.is_err());
        assert!(!successor.state.circle_dir.join("admin.key").exists());
    }

    fn member(
        peer: &str,
        owner: &str,
        age_days: i64,
        admitted: bool,
        connected: bool,
    ) -> Candidate {
        Candidate {
            peer_id: peer.into(),
            owner: owner.into(),
            added_at: Utc::now() - Days::days(age_days),
            admitted,
            connected,
        }
    }

    #[test]
    fn prefers_another_device_of_the_same_owner() {
        let candidates = [
            member("bob-laptop", "bob", 30, true, true),
            member("suzy-air", "suzy", 1, true, true),
        ];
        assert_eq!(
            choose_successor("suzy", &candidates, None).unwrap(),
            "suzy-air"
        );
    }

    #[test]
    fn otherwise_the_longest_standing_member() {
        let candidates = [
            member("carol", "carol", 3, true, true),
            member("bob", "bob", 30, true, true),
        ];
        assert_eq!(choose_successor("suzy", &candidates, None).unwrap(), "bob");
    }

    #[test]
    fn only_admitted_connected_members_qualify() {
        let candidates = [
            member("suzy-offline", "suzy", 30, true, false),
            member("suzy-joining", "suzy", 30, false, true),
            member("bob", "bob", 1, true, true),
        ];
        assert_eq!(choose_successor("suzy", &candidates, None).unwrap(), "bob");
    }

    #[test]
    fn no_one_online_is_an_error_not_a_silent_loss_of_admin() {
        let candidates = [member("bob", "bob", 30, true, false)];
        assert!(choose_successor("suzy", &candidates, None).is_err());
        assert!(choose_successor("suzy", &[], None).is_err());
    }

    #[test]
    fn a_requested_successor_must_qualify_too() {
        let candidates = [
            member("bob", "bob", 30, true, true),
            member("carol", "carol", 30, true, false),
        ];
        assert_eq!(
            choose_successor("suzy", &candidates, Some("bob")).unwrap(),
            "bob"
        );
        assert!(choose_successor("suzy", &candidates, Some("carol")).is_err());
        assert!(choose_successor("suzy", &candidates, Some("dave")).is_err());
    }
}
