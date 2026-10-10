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
pub const DEFAULT_RENDEZVOUS: Option<&str> = Some("relay.enoxian.com");

/// Oldest client a bootstrap server built from this source still serves.
///
/// Published as `min_client_version` on the server's `/version`. Clients older
/// than this tell their user to upgrade instead of failing to connect without
/// saying why — the point being a transport change that old and new clients
/// cannot speak across. `None` while every client release is still served.
pub const MIN_CLIENT_VERSION: Option<&str> = None;

/// Iroh relays run for enoxian. A Circle whose `iroh_relays` is empty uses
/// these together with Iroh's public relays (four regions), so a device homes
/// on whichever is nearest and the rest stand by.
pub const DEFAULT_IROH_RELAYS: &[&str] = &["https://relay.enoxian.com"];
