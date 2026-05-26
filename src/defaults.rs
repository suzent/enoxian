/// Default rendezvous / bootstrap server for cross-internet connectivity.
///
/// Set to `Some("enochian.com")` (or any hostname / IP / full multiaddr) to make
/// every invite automatically WAN-reachable without users having to configure anything.
///
/// Set to `None` to disable — users must explicitly pass `--rendezvous` or configure
/// `rendezvous_addrs` in their circle config.
///
/// The value is resolved at runtime via `GET http://<host>/peer-id` so the peer ID
/// doesn't need to be hard-coded here — it is fetched fresh on each daemon start.
pub const DEFAULT_RENDEZVOUS: Option<&str> = Some("enoxian.com");
