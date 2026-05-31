/// Default rendezvous / bootstrap server for cross-internet connectivity.
///
/// Set to `Some("hostname")` (or any hostname / IP / full multiaddr) to make
/// every invite automatically WAN-reachable without users having to configure anything.
///
/// Set to `None` to disable — users must explicitly pass `--rendezvous` or configure
/// `rendezvous_addrs` in their circle config.
///
/// The value is resolved at runtime via `GET http://<host>/peer-id` so the peer ID
/// doesn't need to be hard-coded here — it is fetched fresh on each daemon start.
pub const DEFAULT_RENDEZVOUS: Option<&str> = Some("enoxian.com");

/// Default relay server for WAN NAT traversal.
///
/// Used as a fallback when `relay_addrs` is not configured on a circle.
/// Peers reserve a circuit slot on this server so they remain reachable
/// from the internet even without a rendezvous server.
///
/// Set to the same host as `DEFAULT_RENDEZVOUS` if the same server runs
/// both services (the default `enoxd --bootstrap` setup does this).
/// Set to `None` to disable automatic relay reservation.
pub const DEFAULT_RELAY: Option<&str> = Some("enoxian.com");
