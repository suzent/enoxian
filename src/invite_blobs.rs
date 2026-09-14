//! Where a short invite's contents live.
//!
//! A full invite is ~300 characters because it carries everything a joiner
//! needs inline: circle id, PSK, expiry, name, admin key, relay and rendezvous
//! addresses, and the grant. A short invite carries none of it. The inviter
//! seals that payload, leaves it here, and the link carries only the key —
//! which is 35 characters, short enough to survive a chat message, a slide, or
//! being read out.
//!
//! # What this is trusted with
//!
//! Nothing, in the same sense as the pairing mailbox. The id is an HKDF output
//! of a key the server never sees, and the body is sealed under a second output
//! of the same key. An operator learns that an invite exists, its size, and when
//! it is fetched — not the circle, not the PSK, not who it is for.
//!
//! Losing a blob is a recoverable failure (the inviter makes another link);
//! serving a wrong one is not possible (the seal would not open). So the things
//! it must actually do are refuse to become unbounded storage and stay cheap to
//! serve, which is what the bounds below are for.
//!
//! # Why this is on disk
//!
//! An invite is good for days. Keeping blobs in memory would quietly invalidate
//! every outstanding short link whenever the bootstrap server restarted, which
//! is the kind of failure nobody would attribute to the right cause.

use std::net::SocketAddr;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use axum::{
    body::Bytes,
    extract::{ConnectInfo, Path, State},
    http::StatusCode,
    routing::post,
    Router,
};
use tokio::sync::Mutex;

use crate::rate_limit::Limiter;

/// How long a blob is kept.
///
/// Longer than any invite TTL the CLI will mint, so the blob outliving the
/// invite is the normal case and expiry is decided by the invite's own
/// timestamp, not by this. A blob whose invite expired is dead weight, and this
/// is when it gets swept.
const TTL: Duration = Duration::from_secs(60 * 60 * 24 * 30);

/// Largest blob accepted. A sealed invite is a few hundred bytes; 8 KiB is
/// generous for one and far too small to be worth abusing as storage.
const MAX_BLOB: usize = 8 * 1024;

/// Blobs held at once. Past this, new ones are refused rather than evicting —
/// dropping a stranger's live invite to make room for another is worse than
/// telling the inviter to try again.
const MAX_BLOBS: usize = 100_000;

/// A blob id is the hex of a 32-byte HKDF output. Anything else is not one.
const ID_LEN: usize = 32;

/// Fetching an invite is a one-shot act, so the budget is tighter than the
/// pairing mailbox's — nobody legitimately asks thirty times in ten seconds.
const RATE_MAX: u32 = 30;
const RATE_WINDOW: Duration = Duration::from_secs(10);
const MAX_TRACKED_SOURCES: usize = 8192;

struct Inner {
    dir: PathBuf,
    limiter: Limiter,
}

#[derive(Clone)]
pub struct BlobState(Arc<Mutex<Inner>>);

impl BlobState {
    /// Store blobs under `dir`, creating it if needed.
    pub fn new(dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("create invite blob dir {}", dir.display()))?;
        Ok(BlobState(Arc::new(Mutex::new(Inner {
            dir,
            limiter: Limiter::new(RATE_MAX, RATE_WINDOW, MAX_TRACKED_SOURCES),
        }))))
    }
}

/// Routes to mount under `/invite`.
pub fn router(state: BlobState) -> Router {
    Router::new()
        .route("/{id}", post(put_blob).get(get_blob))
        .with_state(state)
}

/// Reject an id that is not the shape an HKDF output takes, before it reaches
/// the filesystem. This is also what keeps `..` and `/` out of a path.
fn valid_id(id: &str) -> bool {
    id.len() == ID_LEN * 2 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Drop blobs past their TTL, and report how many remain.
///
/// Swept on write rather than on a timer: writes are rare, and a sweep that
/// only runs when the store is being added to cannot fall behind it.
fn sweep(dir: &FsPath) -> usize {
    let now = SystemTime::now();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut kept = 0usize;
    for entry in entries.flatten() {
        let expired = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| now.duration_since(t).unwrap_or_default() > TTL)
            .unwrap_or(false);
        if expired {
            let _ = std::fs::remove_file(entry.path());
        } else {
            kept += 1;
        }
    }
    kept
}

/// Leave a sealed invite. Write-once: a second write is refused rather than
/// replacing, so a link cannot be repointed at different contents once shared.
async fn put_blob(
    State(state): State<BlobState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(id): Path<String>,
    body: Bytes,
) -> StatusCode {
    if !valid_id(&id) {
        return StatusCode::BAD_REQUEST;
    }
    if body.is_empty() || body.len() > MAX_BLOB {
        return StatusCode::PAYLOAD_TOO_LARGE;
    }

    let mut inner = state.0.lock().await;
    inner.limiter.expire();
    if !inner.limiter.allow(peer.ip().into()) {
        return StatusCode::TOO_MANY_REQUESTS;
    }

    let path = inner.dir.join(&id);
    if path.exists() {
        return StatusCode::CONFLICT;
    }
    if sweep(&inner.dir) >= MAX_BLOBS {
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    match std::fs::write(&path, &body) {
        Ok(()) => StatusCode::CREATED,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Fetch a sealed invite. Non-consuming: an invite link may reasonably be
/// redeemed after a failed attempt, and one-use is enforced where it means
/// something — the grant nonce, burned when the circle admits the joiner.
async fn get_blob(
    State(state): State<BlobState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(id): Path<String>,
) -> (StatusCode, Vec<u8>) {
    if !valid_id(&id) {
        return (StatusCode::BAD_REQUEST, Vec::new());
    }

    let mut inner = state.0.lock().await;
    inner.limiter.expire();
    if !inner.limiter.allow(peer.ip().into()) {
        return (StatusCode::TOO_MANY_REQUESTS, Vec::new());
    }

    let path = inner.dir.join(&id);
    let fresh = path
        .metadata()
        .and_then(|m| m.modified())
        .map(|t| SystemTime::now().duration_since(t).unwrap_or_default() <= TTL)
        .unwrap_or(false);
    if !fresh {
        return (StatusCode::NOT_FOUND, Vec::new());
    }
    match std::fs::read(&path) {
        Ok(bytes) => (StatusCode::OK, bytes),
        Err(_) => (StatusCode::NOT_FOUND, Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> (BlobState, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (BlobState::new(dir.path().to_path_buf()).unwrap(), dir)
    }

    fn id() -> String {
        "a".repeat(64)
    }

    fn peer(n: u8) -> ConnectInfo<SocketAddr> {
        ConnectInfo(SocketAddr::from(([10, 0, 0, n], 40000)))
    }

    async fn put(s: &BlobState, id: &str, body: &[u8]) -> StatusCode {
        put_blob(
            State(s.clone()),
            peer(1),
            Path(id.to_string()),
            Bytes::copy_from_slice(body),
        )
        .await
    }

    async fn get(s: &BlobState, id: &str) -> (StatusCode, Vec<u8>) {
        get_blob(State(s.clone()), peer(2), Path(id.to_string())).await
    }

    #[tokio::test]
    async fn a_blob_comes_back_as_it_went_in() {
        let (s, _d) = state();
        assert_eq!(put(&s, &id(), b"sealed").await, StatusCode::CREATED);
        assert_eq!(get(&s, &id()).await, (StatusCode::OK, b"sealed".to_vec()));
    }

    /// Redeeming must not consume: an invite may be retried after a failure,
    /// and one-use is the grant nonce's job, not this one's.
    #[tokio::test]
    async fn fetching_does_not_consume() {
        let (s, _d) = state();
        put(&s, &id(), b"sealed").await;
        for _ in 0..3 {
            assert_eq!(get(&s, &id()).await.0, StatusCode::OK);
        }
    }

    /// Write-once: a shared link must not be repointable at other contents.
    #[tokio::test]
    async fn a_blob_cannot_be_replaced() {
        let (s, _d) = state();
        assert_eq!(put(&s, &id(), b"first").await, StatusCode::CREATED);
        assert_eq!(put(&s, &id(), b"second").await, StatusCode::CONFLICT);
        assert_eq!(get(&s, &id()).await.1, b"first".to_vec());
    }

    #[tokio::test]
    async fn an_unknown_id_is_not_found() {
        let (s, _d) = state();
        assert_eq!(get(&s, &id()).await.0, StatusCode::NOT_FOUND);
    }

    /// An id is used as a filename, so anything that is not an HKDF output in
    /// hex must be refused before it reaches the filesystem.
    #[tokio::test]
    async fn a_malformed_id_is_refused() {
        let (s, _d) = state();
        for bad in [
            "short",
            &"a".repeat(63),
            &"a".repeat(65),
            &"z".repeat(64),
            "../../etc/passwd",
            &format!("{}/..", "a".repeat(61)),
        ] {
            assert_eq!(put(&s, bad, b"x").await, StatusCode::BAD_REQUEST, "{bad}");
            assert_eq!(get(&s, bad).await.0, StatusCode::BAD_REQUEST, "{bad}");
        }
    }

    /// Path traversal must not be reachable even if an id somehow got through.
    #[tokio::test]
    async fn a_traversing_id_writes_nothing_outside_the_store() {
        let (s, dir) = state();
        let outside = dir.path().parent().unwrap().join("escaped");
        let _ = put(&s, "../escaped", b"x").await;
        assert!(!outside.exists());
    }

    #[tokio::test]
    async fn an_oversized_or_empty_blob_is_refused() {
        let (s, _d) = state();
        assert_eq!(
            put(&s, &id(), &vec![0u8; MAX_BLOB + 1]).await,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(put(&s, &id(), b"").await, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(get(&s, &id()).await.0, StatusCode::NOT_FOUND);
    }

    /// A blob past its TTL is gone, or the store grows forever.
    #[tokio::test]
    async fn an_expired_blob_is_swept() {
        let (s, dir) = state();
        put(&s, &id(), b"sealed").await;

        // Age it past the TTL.
        let path = dir.path().join(id());
        let old = SystemTime::now() - TTL - Duration::from_secs(60);
        filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(old)).unwrap();

        assert_eq!(get(&s, &id()).await.0, StatusCode::NOT_FOUND);

        // And the next write sweeps it off disk rather than leaving it.
        put(&s, &"b".repeat(64), b"other").await;
        assert!(!path.exists());
    }

    /// Blobs survive the process, which is the reason they are files: an
    /// invite is good for days and a restart must not quietly void every
    /// outstanding link.
    #[tokio::test]
    async fn blobs_outlive_the_process() {
        let dir = tempfile::tempdir().unwrap();
        {
            let s = BlobState::new(dir.path().to_path_buf()).unwrap();
            put(&s, &id(), b"sealed").await;
        }
        let reopened = BlobState::new(dir.path().to_path_buf()).unwrap();
        assert_eq!(get(&reopened, &id()).await.1, b"sealed".to_vec());
    }

    #[tokio::test]
    async fn one_source_cannot_ask_without_limit() {
        let (s, _d) = state();
        for i in 0..RATE_MAX {
            assert_ne!(
                get(&s, &id()).await.0,
                StatusCode::TOO_MANY_REQUESTS,
                "refused at {i}"
            );
        }
        assert_eq!(get(&s, &id()).await.0, StatusCode::TOO_MANY_REQUESTS);
    }
}
