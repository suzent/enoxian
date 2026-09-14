//! The mailbox two devices meet in during `enox link`.
//!
//! Mounted on the bootstrap server, which is the one address both machines
//! already know how to reach. It is a dead drop and nothing more: two slots per
//! mailbox, write-once each, read until they expire.
//!
//! # What it is trusted with
//!
//! Nothing. Slot contents are sealed by [`crate::pairing`] under keys derived
//! from a code the server never sees, the mailbox id is an HKDF output rather
//! than the code itself, and a substituted message fails the confirmation
//! number the two users compare. An operator here learns that two devices paired
//! and roughly when — not who they belong to, not the device key or hostname in
//! the offer, and not a single byte of the payload.
//!
//! So the only things it must actually do are refuse to become storage and
//! refuse to become an amplifier, which is what the bounds below are for. Every
//! one of them fails closed.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    body::Bytes,
    extract::{ConnectInfo, Path, State},
    http::StatusCode,
    routing::post,
    Router,
};
use tokio::sync::Mutex;

use crate::rate_limit::Limiter;

/// How long a mailbox lives. Matches the pairing session window — a code is
/// dead by then anyway, so anything still here is litter.
const TTL: Duration = Duration::from_secs(crate::pairing::SESSION_TIMEOUT_SECS);

/// Largest slot we will hold. An offer is a few hundred bytes; a payload is one
/// invite per circle, and invites are ~300 characters each. 64 KiB is room for
/// a couple of hundred circles and still far too small to be worth abusing.
const MAX_SLOT: usize = 64 * 1024;

/// Mailboxes held at once. Past this, new ones are refused rather than evicting
/// live ones — failing a new pairing is recoverable, breaking one already in
/// flight is confusing.
const MAX_MAILBOXES: usize = 1024;

/// A mailbox id is the hex of a 32-byte HKDF output. Anything else is not one.
const ID_LEN: usize = 32;

/// Requests one source may make per [`RATE_WINDOW`].
///
/// A device pairing polls at roughly 1.4 requests a second, so 60 per ten
/// seconds leaves about four times the headroom a legitimate client needs —
/// enough that two colleagues behind one office NAT pairing at the same moment
/// do not throttle each other. See [`crate::rate_limit`] for what this is and
/// is not for.
const RATE_MAX: u32 = 60;
const RATE_WINDOW: Duration = Duration::from_secs(10);

/// Sources tracked for rate limiting at once.
const MAX_TRACKED_SOURCES: usize = 8192;

/// The three slots, each written once by one side and read by the other.
///
/// `hello` carries the source's ephemeral public key and is written first. It
/// exists so the joining device can compute the confirmation number *before*
/// anyone is asked to confirm: without it the source would have to answer "does
/// the other device show the same number?" while the other device still had
/// nothing to show. NIP-AB gets this for free by putting the source's key in
/// the QR code; a code short enough to retype has no room for it.
#[derive(Default)]
struct Mailbox {
    hello: Option<Vec<u8>>,
    offer: Option<Vec<u8>>,
    reply: Option<Vec<u8>>,
}

/// Which slot a request names.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    Hello,
    Offer,
    Reply,
}

impl Mailbox {
    fn slot(&mut self, which: Slot) -> &mut Option<Vec<u8>> {
        match which {
            Slot::Hello => &mut self.hello,
            Slot::Offer => &mut self.offer,
            Slot::Reply => &mut self.reply,
        }
    }
}

struct Inner {
    boxes: HashMap<String, (Mailbox, Instant)>,
    limiter: Limiter,
}

#[derive(Clone)]
pub struct MailboxState(Arc<Mutex<Inner>>);

impl MailboxState {
    pub fn new() -> Self {
        MailboxState(Arc::new(Mutex::new(Inner {
            boxes: HashMap::new(),
            limiter: Limiter::new(RATE_MAX, RATE_WINDOW, MAX_TRACKED_SOURCES),
        })))
    }

    /// Drop everything past its TTL. Called on every request, so the map stays
    /// bounded without a background task to supervise.
    fn expire(inner: &mut Inner) {
        let now = Instant::now();
        inner
            .boxes
            .retain(|_, (_, created)| now.duration_since(*created) < TTL);
        inner.limiter.expire();
    }
}

impl Default for MailboxState {
    fn default() -> Self {
        Self::new()
    }
}

/// Routes to mount under `/pair`.
pub fn router(state: MailboxState) -> Router {
    Router::new()
        .route("/{id}/{slot}", post(put_slot).get(get_slot))
        .with_state(state)
}

/// Reject an id that is not the shape an HKDF output takes, before it can be
/// used as a map key.
fn valid_id(id: &str) -> bool {
    id.len() == ID_LEN * 2 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

fn slot_index(slot: &str) -> Option<Slot> {
    match slot {
        "hello" => Some(Slot::Hello),
        "offer" => Some(Slot::Offer),
        "reply" => Some(Slot::Reply),
        _ => None,
    }
}

/// Write a slot. Write-once: a second write is refused rather than overwriting,
/// so whoever gets there first owns the session and a late arrival cannot
/// replace a message the other side may already have read.
async fn put_slot(
    State(state): State<MailboxState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path((id, slot)): Path<(String, String)>,
    body: Bytes,
) -> StatusCode {
    if !valid_id(&id) {
        return StatusCode::BAD_REQUEST;
    }
    let Some(which) = slot_index(&slot) else {
        return StatusCode::NOT_FOUND;
    };
    if body.is_empty() || body.len() > MAX_SLOT {
        return StatusCode::PAYLOAD_TOO_LARGE;
    }

    let mut inner = state.0.lock().await;
    MailboxState::expire(&mut inner);
    if !inner.limiter.allow(peer.ip().into()) {
        return StatusCode::TOO_MANY_REQUESTS;
    }

    // Only a brand-new mailbox counts against the cap: refusing a reply to a
    // pairing already under way would strand it half-done.
    let exists = inner.boxes.contains_key(&id);
    if !exists && inner.boxes.len() >= MAX_MAILBOXES {
        return StatusCode::SERVICE_UNAVAILABLE;
    }

    let entry = inner
        .boxes
        .entry(id)
        .or_insert_with(|| (Mailbox::default(), Instant::now()));
    let target = entry.0.slot(which);
    if target.is_some() {
        return StatusCode::CONFLICT;
    }
    *target = Some(body.to_vec());
    StatusCode::CREATED
}

/// Read a slot. `404` until the other side has written — the caller polls.
///
/// Reading does not consume: the source may poll the offer slot more than once
/// across a retry, and a consuming read would lose the message to whichever
/// poll happened to land first.
async fn get_slot(
    State(state): State<MailboxState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path((id, slot)): Path<(String, String)>,
) -> (StatusCode, Vec<u8>) {
    if !valid_id(&id) {
        return (StatusCode::BAD_REQUEST, Vec::new());
    }
    let Some(which) = slot_index(&slot) else {
        return (StatusCode::NOT_FOUND, Vec::new());
    };

    let mut inner = state.0.lock().await;
    MailboxState::expire(&mut inner);
    if !inner.limiter.allow(peer.ip().into()) {
        return (StatusCode::TOO_MANY_REQUESTS, Vec::new());
    }

    match inner.boxes.get_mut(&id) {
        Some((mailbox, _)) => match mailbox.slot(which) {
            Some(bytes) => (StatusCode::OK, bytes.clone()),
            None => (StatusCode::NOT_FOUND, Vec::new()),
        },
        None => (StatusCode::NOT_FOUND, Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> String {
        "a".repeat(64)
    }

    fn peer(n: u8) -> ConnectInfo<SocketAddr> {
        ConnectInfo(SocketAddr::from(([10, 0, 0, n], 40000)))
    }

    /// A distinct source per index — for the tests about how many *mailboxes*
    /// the server holds, which in life come from that many different devices
    /// and must not be throttled as though one host made them all.
    fn peer_seq(i: usize) -> ConnectInfo<SocketAddr> {
        let [_, b, c, d] = (i as u32).to_be_bytes();
        ConnectInfo(SocketAddr::from(([10, b, c, d], 40000)))
    }

    async fn put(state: &MailboxState, id: &str, slot: &str, body: &[u8]) -> StatusCode {
        put_from(state, peer(1), id, slot, body).await
    }

    async fn put_from(
        state: &MailboxState,
        from: ConnectInfo<SocketAddr>,
        id: &str,
        slot: &str,
        body: &[u8],
    ) -> StatusCode {
        put_slot(
            State(state.clone()),
            from,
            Path((id.to_string(), slot.to_string())),
            Bytes::copy_from_slice(body),
        )
        .await
    }

    async fn get(state: &MailboxState, id: &str, slot: &str) -> (StatusCode, Vec<u8>) {
        get_slot(
            State(state.clone()),
            peer(1),
            Path((id.to_string(), slot.to_string())),
        )
        .await
    }

    #[tokio::test]
    async fn a_slot_written_by_one_side_is_read_by_the_other() {
        let state = MailboxState::new();
        assert_eq!(
            put(&state, &id(), "offer", b"sealed").await,
            StatusCode::CREATED
        );
        assert_eq!(
            get(&state, &id(), "offer").await,
            (StatusCode::OK, b"sealed".to_vec())
        );
    }

    /// The reader polls, so an unwritten slot has to be an ordinary "not yet"
    /// rather than an error it would give up on.
    #[test]
    fn an_unwritten_slot_reads_as_not_found() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let state = MailboxState::new();
            assert_eq!(get(&state, &id(), "reply").await.0, StatusCode::NOT_FOUND);
            put(&state, &id(), "offer", b"x").await;
            assert_eq!(get(&state, &id(), "reply").await.0, StatusCode::NOT_FOUND);
        });
    }

    /// Write-once. A second writer must not be able to swap a message the other
    /// side may already have acted on.
    #[tokio::test]
    async fn a_slot_cannot_be_overwritten() {
        let state = MailboxState::new();
        assert_eq!(
            put(&state, &id(), "offer", b"first").await,
            StatusCode::CREATED
        );
        assert_eq!(
            put(&state, &id(), "offer", b"second").await,
            StatusCode::CONFLICT
        );
        assert_eq!(get(&state, &id(), "offer").await.1, b"first".to_vec());
    }

    /// Reading must not consume — the source polls the offer slot repeatedly
    /// and every poll has to see the same message.
    #[tokio::test]
    async fn reading_a_slot_leaves_it_in_place() {
        let state = MailboxState::new();
        put(&state, &id(), "offer", b"sealed").await;
        for _ in 0..3 {
            assert_eq!(get(&state, &id(), "offer").await.0, StatusCode::OK);
        }
    }

    /// The slots are independent: one being written must not make another
    /// readable, or a side would end up reading its own message back.
    #[tokio::test]
    async fn the_slots_do_not_bleed_into_each_other() {
        let state = MailboxState::new();
        put(&state, &id(), "hello", b"source-key").await;
        put(&state, &id(), "offer", b"from-target").await;
        put(&state, &id(), "reply", b"from-source").await;
        assert_eq!(get(&state, &id(), "hello").await.1, b"source-key".to_vec());
        assert_eq!(get(&state, &id(), "offer").await.1, b"from-target".to_vec());
        assert_eq!(get(&state, &id(), "reply").await.1, b"from-source".to_vec());
    }

    #[tokio::test]
    async fn mailboxes_are_separate() {
        let state = MailboxState::new();
        let other = "b".repeat(64);
        put(&state, &id(), "offer", b"mine").await;
        assert_eq!(get(&state, &other, "offer").await.0, StatusCode::NOT_FOUND);
    }

    /// The id a real code derives must be one this server accepts. These are
    /// separate modules, and when they disagreed about the length every `enox
    /// link` failed with a bare 400 — caught only by running the two together.
    #[tokio::test]
    async fn a_real_pairing_code_produces_an_id_this_accepts() {
        let code = crate::pairing::Code::generate().unwrap();
        let id = code.mailbox_id();
        assert!(
            valid_id(&id),
            "a code derived an id the mailbox refuses: {id}"
        );

        let state = MailboxState::new();
        assert_eq!(put(&state, &id, "hello", b"key").await, StatusCode::CREATED);
    }

    /// An id that is not an HKDF output in hex was not produced by a code, and
    /// must not reach the map as a key.
    #[tokio::test]
    async fn a_malformed_id_is_refused() {
        let state = MailboxState::new();
        for bad in ["short", &"a".repeat(63), &"a".repeat(65), &"z".repeat(64)] {
            assert_eq!(
                put(&state, bad, "offer", b"x").await,
                StatusCode::BAD_REQUEST
            );
            assert_eq!(get(&state, bad, "offer").await.0, StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn an_unknown_slot_name_is_refused() {
        let state = MailboxState::new();
        assert_eq!(
            put(&state, &id(), "payload", b"x").await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            get(&state, &id(), "../offer").await.0,
            StatusCode::NOT_FOUND
        );
    }

    /// The size bound is what stops the mailbox being used as storage.
    #[tokio::test]
    async fn an_oversized_or_empty_slot_is_refused() {
        let state = MailboxState::new();
        let big = vec![0u8; MAX_SLOT + 1];
        assert_eq!(
            put(&state, &id(), "offer", &big).await,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            put(&state, &id(), "offer", b"").await,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        // The refused writes must not have created the mailbox.
        assert_eq!(get(&state, &id(), "offer").await.0, StatusCode::NOT_FOUND);
    }

    /// Past the cap, a new mailbox is refused — but one already in flight can
    /// still receive its reply, or the cap would strand live pairings.
    #[tokio::test]
    async fn the_mailbox_cap_refuses_new_sessions_not_live_ones() {
        let state = MailboxState::new();
        for i in 0..MAX_MAILBOXES {
            let id = format!("{i:064x}");
            assert_eq!(
                put_from(&state, peer_seq(i), &id, "offer", b"x").await,
                StatusCode::CREATED
            );
        }
        let fresh = "f".repeat(64);
        assert_eq!(
            put_from(&state, peer_seq(9_000), &fresh, "offer", b"x").await,
            StatusCode::SERVICE_UNAVAILABLE
        );

        // A session that already has a mailbox still completes.
        let live = format!("{:064x}", 0);
        assert_eq!(
            put_from(&state, peer_seq(9_001), &live, "reply", b"x").await,
            StatusCode::CREATED
        );
    }

    /// The budget must refuse a source that keeps asking, or the mailbox is a
    /// free way to hammer the bootstrap host.
    #[tokio::test]
    async fn one_source_cannot_ask_without_limit() {
        let state = MailboxState::new();
        for i in 0..RATE_MAX {
            let status = get(&state, &id(), "offer").await.0;
            assert_ne!(status, StatusCode::TOO_MANY_REQUESTS, "refused at {i}");
        }
        assert_eq!(
            get(&state, &id(), "offer").await.0,
            StatusCode::TOO_MANY_REQUESTS
        );
    }

    /// One noisy source must not take the budget of an unrelated one, or a
    /// single bad actor would stop everyone else pairing.
    #[tokio::test]
    async fn the_budget_is_per_source() {
        let state = MailboxState::new();
        for _ in 0..RATE_MAX + 5 {
            let _ = put_from(&state, peer(1), &id(), "offer", b"x").await;
        }
        assert_eq!(
            put_from(&state, peer(2), &"b".repeat(64), "offer", b"x").await,
            StatusCode::CREATED
        );
    }

    /// A real pairing polls for the whole window. The budget has to be loose
    /// enough that an honest client never trips it.
    #[tokio::test]
    async fn an_honest_client_stays_inside_the_budget() {
        // Two sides, polling at ~1.4 req/s, over one window.
        let polls_per_window = (RATE_WINDOW.as_secs_f64() * 1.4).ceil() as u32;
        assert!(
            polls_per_window * 2 < RATE_MAX,
            "a pairing makes ~{polls_per_window} polls per side per window, \
             against a budget of {RATE_MAX}"
        );
    }

    /// The window must roll, or a source is locked out for as long as the
    /// server is up. The limiter's own behaviour is covered in
    /// `crate::rate_limit`; this checks the mailbox actually consults it.
    #[tokio::test]
    async fn the_budget_refills_after_the_window() {
        let state = MailboxState::new();
        for _ in 0..RATE_MAX + 1 {
            let _ = get(&state, &id(), "offer").await;
        }
        assert_eq!(
            get(&state, &id(), "offer").await.0,
            StatusCode::TOO_MANY_REQUESTS
        );

        state.0.lock().await.limiter.force_expire_all();
        assert_ne!(
            get(&state, &id(), "offer").await.0,
            StatusCode::TOO_MANY_REQUESTS
        );
    }

    /// Nothing may outlive the pairing window, or the server becomes storage.
    #[tokio::test]
    async fn expired_mailboxes_are_dropped() {
        let state = MailboxState::new();
        put(&state, &id(), "offer", b"x").await;

        {
            let mut inner = state.0.lock().await;
            let entry = inner.boxes.get_mut(&id()).unwrap();
            entry.1 = Instant::now() - TTL - Duration::from_secs(1);
        }

        assert_eq!(get(&state, &id(), "offer").await.0, StatusCode::NOT_FOUND);
        assert!(state.0.lock().await.boxes.is_empty());
    }

    /// An expired mailbox frees its slot against the cap, so a busy server
    /// recovers on its own rather than staying wedged for as long as it is up.
    #[tokio::test]
    async fn expiry_releases_capacity() {
        let state = MailboxState::new();
        for i in 0..MAX_MAILBOXES {
            put_from(&state, peer_seq(i), &format!("{i:064x}"), "offer", b"x").await;
        }
        {
            let mut inner = state.0.lock().await;
            for (_, entry) in inner.boxes.iter_mut() {
                entry.1 = Instant::now() - TTL - Duration::from_secs(1);
            }
        }
        assert_eq!(
            put_from(&state, peer_seq(9_002), &"f".repeat(64), "offer", b"x").await,
            StatusCode::CREATED
        );
    }
}
