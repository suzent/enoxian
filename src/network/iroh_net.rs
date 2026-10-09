//! The Iroh transport, behind the `iroh-transport` feature while in development.
//!
//! One endpoint per Circle, keyed by the Circle's Ed25519 key, so a member's
//! EndpointId is the same public key as its PeerId and the two convert both
//! ways. One connection per pair of peers carries every protocol: a single
//! ALPN, and each bidirectional stream opens with a one-byte tag naming the
//! protocol and a proof, both ways, that the peer holds the Circle secret.
//!
//! Admission, per stream: the Circle proof for everything (it stands in for
//! the libp2p pnet PSK, but covers relayed paths too), plus a leaf in our MLS
//! group for the content protocols. MLS bootstrap needs only the proof: it is
//! how a joiner gets its Welcome.
//!
//! Connections: the member with the lower PeerId dials. A joiner also dials
//! the peers its invite named, since they do not know it yet. If two
//! connections to one peer appear anyway, the one dialed by the lower PeerId
//! stays; if the same side dialed both, it lost the first, and the newer stays.

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use futures::future::BoxFuture;
use iroh::{
    endpoint::{
        presets, Connection, PortmapperConfig, QuicTransportConfig, RecvStream, SendStream,
    },
    Endpoint, EndpointAddr, EndpointId, RelayMap, RelayMode, RelayUrl, SecretKey, TransportAddr,
};
use libp2p::{multiaddr::Protocol, Multiaddr, PeerId};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::net::{self, NetHandle, PeerStream, Proto};
use crate::{
    config::CircleConfig,
    lifecycle::{mark_peer_offline, member_peer_ids, PeerRedials, RECONNECT_INTERVAL},
    state::{classify_ip, AppState, ConnectionKind},
};

/// Every enoxian stream rides this ALPN; the first byte says which protocol.
pub const ALPN: &[u8] = b"enoxian/3";
const PROOF_LEN: usize = 32;
/// How long a stream may take to show its tag and proof.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// A refused stream (not yet admitted, or behind on MLS) is retried this often.
const REOPEN_INTERVAL: Duration = Duration::from_secs(5);
/// A connection with no traffic for this long is gone. Iroh sends keep-alives
/// every 5s; its own default lets a dead relayed connection linger for 30s.
const IDLE_TIMEOUT: Duration = Duration::from_secs(15);
/// After losing a peer we dial, try again this soon rather than at the sweep.
const REDIAL_AFTER: Duration = Duration::from_secs(2);

fn tag(proto: Proto) -> u8 {
    match proto {
        Proto::MlsBootstrap => 1,
        Proto::AdminHandover => 2,
        Proto::Sync => 3,
        Proto::Proposals => 4,
        Proto::Events => 5,
    }
}

fn from_tag(tag: u8) -> Option<Proto> {
    Proto::ALL
        .into_iter()
        .find(|proto| self::tag(*proto) == tag)
}

// ── Identity ─────────────────────────────────────────────────────────────────

pub fn endpoint_id(peer: &PeerId) -> Result<EndpointId> {
    let key = libp2p::identity::PublicKey::try_decode_protobuf(peer.as_ref().digest())
        .context("peer id carries no inline public key")?
        .try_into_ed25519()
        .context("peer id is not an Ed25519 key")?;
    Ok(EndpointId::from_bytes(&key.to_bytes())?)
}

pub fn peer_id(id: &EndpointId) -> Result<PeerId> {
    let key = libp2p::identity::ed25519::PublicKey::try_from_bytes(id.as_bytes())?;
    Ok(libp2p::identity::PublicKey::from(key).to_peer_id())
}

fn secret_key(keypair: &libp2p::identity::Keypair) -> Result<SecretKey> {
    let ed = keypair
        .clone()
        .try_into_ed25519()
        .context("circle key is not Ed25519")?;
    let bytes: [u8; 32] = ed.secret().as_ref().try_into()?;
    Ok(SecretKey::from_bytes(&bytes))
}

// ── Circle proof ─────────────────────────────────────────────────────────────

/// Proof that `sender` holds the Circle secret, bound to this pair of peers so
/// it cannot be replayed to anyone else. HKDF-SHA256 is HMAC underneath.
fn proof(psk: &[u8; 32], circle_id: &str, sender: &PeerId, receiver: &PeerId) -> [u8; PROOF_LEN] {
    let mut info = Vec::new();
    for part in [
        circle_id.as_bytes(),
        &sender.to_bytes(),
        &receiver.to_bytes(),
    ] {
        info.extend_from_slice(&(part.len() as u32).to_be_bytes());
        info.extend_from_slice(part);
    }
    let mut out = [0u8; PROOF_LEN];
    hkdf::Hkdf::<sha2::Sha256>::new(Some(b"enoxian-circle-proof-v1"), psk)
        .expand(&info, &mut out)
        .expect("32 bytes is a valid HKDF-SHA256 length");
    out
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ── Transport ────────────────────────────────────────────────────────────────

struct Live {
    connection: Connection,
    dialer: PeerId,
}

struct Inner {
    endpoint: Endpoint,
    state: AppState,
    local: PeerId,
    psk: [u8; 32],
    relays: Vec<RelayUrl>,
    connections: Mutex<HashMap<PeerId, Live>>,
    /// Direct-address hints for peers, from invites and configured addresses.
    hints: Mutex<HashMap<PeerId, Vec<SocketAddr>>>,
    /// Peers our invite named: dialed whatever their PeerId.
    invited: Vec<PeerId>,
}

/// Opens streams for protocols that start one themselves (admin handover).
#[derive(Clone)]
pub struct IrohNet(Arc<Inner>);

impl NetHandle for IrohNet {
    fn open(&self, peer: PeerId, proto: Proto) -> BoxFuture<'static, Result<PeerStream>> {
        let net = self.clone();
        Box::pin(async move {
            let connection = match net.connection(&peer) {
                Some(connection) => connection,
                None => net.dial(peer).await?,
            };
            net.open_on(&connection, peer, proto).await
        })
    }
}

/// Bring the Circle up on Iroh. Returns once the endpoint is bound; the
/// connection loops run until `token` is cancelled.
pub async fn spawn(
    config: &CircleConfig,
    state: AppState,
    keypair: &libp2p::identity::Keypair,
    psk: [u8; 32],
    token: CancellationToken,
) -> Result<()> {
    let relay_map = relay_map(&config.iroh_relays);
    let relays: Vec<RelayUrl> = relay_map.urls();
    // Minimal, not N0: nothing is published to Iroh's DNS. Members find each
    // other by id through the relays, which every dial lists in full.
    let mut builder = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Custom(relay_map))
        .secret_key(secret_key(keypair)?)
        .alpns(vec![ALPN.to_vec()])
        .transport_config(
            QuicTransportConfig::builder()
                .max_idle_timeout(Some(IDLE_TIMEOUT.try_into()?))
                .build(),
        )
        // Gateway probing can raise the macOS firewall prompt.
        .portmapper_config(PortmapperConfig::Disabled);
    if config.force_relay {
        builder = builder.clear_ip_transports();
    }
    let endpoint = builder.bind().await.context("binding the Iroh endpoint")?;
    let local: PeerId = state.peer_id.parse()?;
    info!(
        "[{}] Iroh endpoint {} (peer {local})",
        state.circle_id,
        endpoint.id()
    );

    let mut hints: HashMap<PeerId, Vec<SocketAddr>> = HashMap::new();
    let mut invited = Vec::new();
    for addr in &config.peers {
        if let Some((peer, direct)) = parse_peer_addr(addr) {
            hints.entry(peer).or_default().extend(direct);
            invited.push(peer);
        }
    }

    let net = IrohNet(Arc::new(Inner {
        endpoint: endpoint.clone(),
        state: state.clone(),
        local,
        psk,
        relays,
        connections: Mutex::new(HashMap::new()),
        hints: Mutex::new(hints),
        invited: invited.clone(),
    }));
    state.set_net(Arc::new(net.clone()));

    tokio::spawn(net.clone().publish_addresses(token.clone()));
    tokio::spawn(net.clone().accept_loop(token.clone()));
    tokio::spawn(net.clone().dial_loop(invited, token.clone()));
    tokio::spawn(net.clone().sync_ended_loop(token.clone()));
    Ok(())
}

/// The relays a Circle uses: its own `iroh_relays` when it lists any,
/// otherwise ours ([`crate::defaults::DEFAULT_IROH_RELAYS`]) plus Iroh's
/// public relays. Each device homes on the one nearest it; dialing lists them
/// all, since members of one Circle home on different relays.
pub fn relay_map(configured: &[String]) -> RelayMap {
    let parse = |url: &str| match url.parse::<RelayUrl>() {
        Ok(url) => Some(url),
        Err(error) => {
            warn!("[iroh] ignoring relay '{url}': {error}");
            None
        }
    };
    let own: Vec<RelayUrl> = configured.iter().filter_map(|url| parse(url)).collect();
    if !own.is_empty() {
        return RelayMap::from_iter(own);
    }
    let map = RelayMap::from_iter(
        crate::defaults::DEFAULT_IROH_RELAYS
            .iter()
            .filter_map(|url| parse(url)),
    );
    map.extend(&iroh::defaults::prod::default_relay_map());
    map
}

/// `/ip4/<ip>/udp/<port>/quic-v1/p2p/<peer>` style addresses: the peer, and
/// any IP + UDP port in it as a direct-address hint.
fn parse_peer_addr(addr: &str) -> Option<(PeerId, Vec<SocketAddr>)> {
    let addr: Multiaddr = addr.parse().ok()?;
    let mut ip = None;
    let mut udp = None;
    let mut peer = None;
    for part in addr.iter() {
        match part {
            Protocol::Ip4(v4) => ip = Some(std::net::IpAddr::from(v4)),
            Protocol::Ip6(v6) => ip = Some(std::net::IpAddr::from(v6)),
            Protocol::Udp(port) => udp = Some(port),
            Protocol::P2p(id) => peer = Some(id),
            _ => {}
        }
    }
    let direct = match (ip, udp) {
        (Some(ip), Some(port)) => vec![SocketAddr::new(ip, port)],
        _ => Vec::new(),
    };
    Some((peer?, direct))
}

impl IrohNet {
    fn circle(&self) -> &str {
        &self.0.state.circle_id
    }

    fn connection(&self, peer: &PeerId) -> Option<Connection> {
        let connections = self.0.connections.lock().unwrap();
        connections
            .get(peer)
            .filter(|live| live.connection.close_reason().is_none())
            .map(|live| live.connection.clone())
    }

    fn address(&self, peer: &PeerId) -> Result<EndpointAddr> {
        let mut addr = EndpointAddr::new(endpoint_id(peer)?);
        for relay in &self.0.relays {
            addr = addr.with_relay_url(relay.clone());
        }
        if let Some(direct) = self.0.hints.lock().unwrap().get(peer) {
            for ip in direct {
                addr = addr.with_ip_addr(*ip);
            }
        }
        Ok(addr)
    }

    async fn dial(&self, peer: PeerId) -> Result<Connection> {
        let connection = self
            .0
            .endpoint
            .connect(self.address(&peer)?, ALPN)
            .await
            .with_context(|| format!("connecting to {peer}"))?;
        self.adopt(connection, true)?
            .context("kept the existing connection")
    }

    /// Keep a new connection unless one dialed by the lower PeerId is already
    /// up. Returns the connection that stays, or `None` if this one was closed.
    fn adopt(&self, connection: Connection, we_dialed: bool) -> Result<Option<Connection>> {
        let remote = peer_id(&connection.remote_id())?;
        let dialer = if we_dialed { self.0.local } else { remote };
        let mut connections = self.0.connections.lock().unwrap();
        if let Some(existing) = connections.get(&remote) {
            if existing.connection.close_reason().is_none()
                && existing.dialer.to_bytes() < dialer.to_bytes()
            {
                connection.close(0u32.into(), b"duplicate");
                return Ok(None);
            }
            existing.connection.close(0u32.into(), b"duplicate");
        }
        connections.insert(
            remote,
            Live {
                connection: connection.clone(),
                dialer,
            },
        );
        drop(connections);
        info!(
            "[{}] Iroh connected: {remote} ({})",
            self.circle(),
            if we_dialed { "dialed" } else { "accepted" }
        );
        tokio::spawn(self.clone().watch(connection.clone(), remote));
        if we_dialed {
            for proto in Proto::ON_CONNECT {
                tokio::spawn(self.clone().run_outbound(connection.clone(), remote, proto));
            }
        }
        Ok(Some(connection))
    }

    /// Keep the connection's route on the status page current; when it closes,
    /// forget it, and mark the peer offline if it was the last one.
    async fn watch(self, connection: Connection, peer: PeerId) {
        let key = connection.stable_id().to_string();
        let mut last = None;
        loop {
            if let Some((kind, address)) = route(&connection) {
                if last.as_ref() != Some(&(kind, address.clone())) {
                    self.0.state.record_peer_connection(
                        peer.to_string(),
                        key.clone(),
                        kind,
                        address.clone(),
                    );
                    last = Some((kind, address));
                }
            }
            tokio::select! {
                _ = connection.closed() => break,
                _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            }
        }
        self.0.state.remove_peer_connection(&peer.to_string(), &key);
        let still_connected = {
            let mut connections = self.0.connections.lock().unwrap();
            let current = connections
                .get(&peer)
                .is_some_and(|live| live.connection.stable_id() == connection.stable_id());
            if current {
                connections.remove(&peer);
            }
            connections.contains_key(&peer)
        };
        if !still_connected {
            info!("[{}] Iroh disconnected: {peer}", self.circle());
            mark_peer_offline(&self.0.state, &peer.to_string());
            if self.dials(&peer) {
                tokio::time::sleep(REDIAL_AFTER).await;
                if self.connection(&peer).is_none() && !self.0.endpoint.is_closed() {
                    if let Err(error) = self.dial(peer).await {
                        debug!("[{}] redial {peer}: {error:#}", self.circle());
                    }
                }
            }
        }
    }

    /// Whether this side is the one that dials `peer`.
    fn dials(&self, peer: &PeerId) -> bool {
        peer.to_bytes() > self.0.local.to_bytes() || self.0.invited.contains(peer)
    }

    /// Open `proto` on a connection we dialed and run it; while the peer
    /// refuses it (not admitted yet, or behind on MLS), try again.
    async fn run_outbound(self, connection: Connection, peer: PeerId, proto: Proto) {
        loop {
            if connection.close_reason().is_some() {
                return;
            }
            match self.open_on(&connection, peer, proto).await {
                Ok(stream) => {
                    net::serve(proto, peer, stream, self.0.state.clone(), true).await;
                    return;
                }
                Err(error) => {
                    debug!(
                        "[{}] {peer} refused {}: {error}",
                        proto.name(),
                        self.circle()
                    );
                    tokio::select! {
                        _ = connection.closed() => return,
                        _ = tokio::time::sleep(REOPEN_INTERVAL) => {}
                    }
                }
            }
        }
    }

    async fn open_on(
        &self,
        connection: &Connection,
        peer: PeerId,
        proto: Proto,
    ) -> Result<PeerStream> {
        let (mut send, mut recv) = connection.open_bi().await?;
        tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            send.write_all(&[tag(proto)]).await?;
            send.write_all(&proof(&self.0.psk, self.circle(), &self.0.local, &peer))
                .await?;
            let mut theirs = [0u8; PROOF_LEN];
            recv.read_exact(&mut theirs)
                .await
                .context("peer refused the stream")?;
            anyhow::ensure!(
                same(
                    &theirs,
                    &proof(&self.0.psk, self.circle(), &peer, &self.0.local)
                ),
                "peer failed the Circle proof"
            );
            anyhow::Ok(())
        })
        .await
        .context("stream handshake timed out")??;
        Ok(Box::new(tokio::io::join(recv, send)))
    }

    async fn accept_loop(self, token: CancellationToken) {
        loop {
            let incoming = tokio::select! {
                _ = token.cancelled() => break,
                incoming = self.0.endpoint.accept() => match incoming {
                    Some(incoming) => incoming,
                    None => break,
                },
            };
            let net = self.clone();
            tokio::spawn(async move {
                let connection = match incoming.await {
                    Ok(connection) => connection,
                    Err(error) => {
                        debug!("[{}] Iroh handshake failed: {error}", net.circle());
                        return;
                    }
                };
                let Ok(peer) = peer_id(&connection.remote_id()) else {
                    connection.close(1u32.into(), b"bad key");
                    return;
                };
                // A removed device gets no connection at all.
                if net.0.state.is_peer_removed(&peer.to_string()) {
                    connection.close(2u32.into(), b"removed");
                    return;
                }
                let Ok(Some(connection)) = net.adopt(connection, false) else {
                    return;
                };
                while let Ok((send, recv)) = connection.accept_bi().await {
                    tokio::spawn(net.clone().serve_inbound(peer, send, recv));
                }
            });
        }
        mark_peer_offline_on_shutdown(&self.0.state);
        self.0.endpoint.close().await;
    }

    async fn serve_inbound(self, peer: PeerId, mut send: SendStream, mut recv: RecvStream) {
        let admitted = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            let mut header = [0u8; 1 + PROOF_LEN];
            recv.read_exact(&mut header).await?;
            let proto = from_tag(header[0]).context("unknown protocol tag")?;
            anyhow::ensure!(
                same(
                    &header[1..],
                    &proof(&self.0.psk, self.circle(), &peer, &self.0.local)
                ),
                "failed the Circle proof"
            );
            if proto != Proto::MlsBootstrap && !self.in_group(&peer).await {
                anyhow::bail!("not in our MLS group (yet)");
            }
            send.write_all(&proof(&self.0.psk, self.circle(), &self.0.local, &peer))
                .await?;
            anyhow::Ok(proto)
        })
        .await;
        match admitted {
            Ok(Ok(proto)) => {
                net::serve(
                    proto,
                    peer,
                    Box::new(tokio::io::join(recv, send)),
                    self.0.state.clone(),
                    false,
                )
                .await;
            }
            Ok(Err(error)) => {
                debug!("[{}] refused a stream from {peer}: {error}", self.circle());
                let _ = send.reset(1u32.into());
            }
            Err(_) => {
                let _ = send.reset(1u32.into());
            }
        }
    }

    async fn in_group(&self, peer: &PeerId) -> bool {
        self.0
            .state
            .mls
            .lock()
            .await
            .group
            .as_ref()
            .is_some_and(|group| group.leaf_index_for_peer(&peer.to_string()).is_some())
    }

    /// Dial members we are meant to dial (lower PeerId dials) and the peers
    /// our invite named, on the shared retry budget.
    async fn dial_loop(self, invited: Vec<PeerId>, token: CancellationToken) {
        let mut redials = PeerRedials::default();
        let mut ticks = tokio::time::interval(RECONNECT_INTERVAL);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = token.cancelled() => return,
                _ = ticks.tick() => {}
            }
            let mut targets: Vec<PeerId> = member_peer_ids(&self.0.state)
                .into_iter()
                .filter(|peer| self.dials(peer))
                .collect();
            targets.extend(invited.iter().copied());
            targets.sort_by_key(|peer| peer.to_bytes());
            targets.dedup();
            let now = Instant::now();
            for peer in targets {
                if peer == self.0.local
                    || self.connection(&peer).is_some()
                    || self.0.state.is_peer_removed(&peer.to_string())
                    || !redials.allow(peer, now)
                {
                    continue;
                }
                let net = self.clone();
                tokio::spawn(async move {
                    if let Err(error) = net.dial(peer).await {
                        debug!("[{}] dial {peer}: {error:#}", net.circle());
                        net.0.state.record_conn_error(format!("{peer}: {error:#}"));
                    }
                });
            }
            for peer in self.0.connections.lock().unwrap().keys() {
                redials.connected(*peer, now);
            }
        }
    }

    /// A failed sync session closes the connection, as on libp2p, so the
    /// dialer reconnects and every stream starts over.
    async fn sync_ended_loop(self, token: CancellationToken) {
        let mut ended = self.0.state.sync_ended.subscribe();
        loop {
            let peers = tokio::select! {
                _ = token.cancelled() => return,
                event = ended.recv() => match event {
                    Ok(peer) => vec![peer],
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => member_peer_ids(&self.0.state),
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                },
            };
            for peer in peers {
                if self.0.state.sync_sessions.get(&peer.to_string()).is_some() {
                    continue;
                }
                if let Some(connection) = self.connection(&peer) {
                    info!(
                        "[{}] closing connection to {peer}: its sync session ended",
                        self.circle()
                    );
                    connection.close(0u32.into(), b"sync ended");
                }
            }
        }
    }

    /// Show this endpoint's reachable addresses where libp2p's listen
    /// addresses went, as multiaddrs carrying our PeerId, so invites built
    /// from them name us and a direct-address hint.
    async fn publish_addresses(self, token: CancellationToken) {
        let mut ticks = tokio::time::interval(Duration::from_secs(10));
        loop {
            tokio::select! {
                _ = token.cancelled() => return,
                _ = ticks.tick() => {}
            }
            let addrs: Vec<String> = self
                .0
                .endpoint
                .addr()
                .ip_addrs()
                .map(|addr| {
                    let ip = match addr.ip() {
                        std::net::IpAddr::V4(v4) => format!("/ip4/{v4}"),
                        std::net::IpAddr::V6(v6) => format!("/ip6/{v6}"),
                    };
                    format!("{ip}/udp/{}/quic-v1/p2p/{}", addr.port(), self.0.local)
                })
                .collect();
            if let Ok(mut listen) = self.0.state.p2p_listen_addrs.write() {
                *listen = addrs;
            }
        }
    }
}

/// The selected path, as the status page names routes.
fn route(connection: &Connection) -> Option<(ConnectionKind, String)> {
    let paths = connection.paths();
    let selected = paths.iter().find(|path| path.is_selected())?;
    Some(match selected.remote_addr() {
        TransportAddr::Relay(url) => (ConnectionKind::Relay, url.to_string()),
        TransportAddr::Ip(addr) => (classify_ip(addr.ip()), addr.to_string()),
        other => (ConnectionKind::Public, format!("{other:?}")),
    })
}

fn mark_peer_offline_on_shutdown(state: &AppState) {
    crate::presence::write_offline(state, &state.agent_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_ids_and_endpoint_ids_convert_both_ways() {
        for _ in 0..50 {
            let keypair = libp2p::identity::Keypair::generate_ed25519();
            let peer = keypair.public().to_peer_id();
            let id = endpoint_id(&peer).unwrap();
            assert_eq!(peer_id(&id).unwrap(), peer);
            assert_eq!(secret_key(&keypair).unwrap().public(), id);
        }
    }

    #[test]
    fn the_circle_proof_binds_secret_circle_and_both_peers() {
        let (a, b, c) = (PeerId::random(), PeerId::random(), PeerId::random());
        let psk = [7u8; 32];
        let base = proof(&psk, "circle", &a, &b);
        assert_eq!(base, proof(&psk, "circle", &a, &b));
        assert_ne!(base, proof(&[8u8; 32], "circle", &a, &b), "other secret");
        assert_ne!(base, proof(&psk, "other", &a, &b), "other circle");
        assert_ne!(base, proof(&psk, "circle", &b, &a), "direction");
        assert_ne!(base, proof(&psk, "circle", &a, &c), "other receiver");
    }

    #[test]
    fn an_empty_relay_list_means_ours_plus_the_public_ones() {
        let urls: Vec<String> = relay_map(&[])
            .urls::<Vec<RelayUrl>>()
            .iter()
            .map(|url| url.to_string())
            .collect();
        assert!(
            urls.contains(&"https://relay.enoxian.com/".to_string()),
            "{urls:?}"
        );
        assert_eq!(urls.len(), 1 + 4, "ours and four public regions: {urls:?}");
    }

    #[test]
    fn a_configured_relay_list_is_used_as_is() {
        let urls = relay_map(&["https://relay.example".into(), "not a url".into()])
            .urls::<Vec<RelayUrl>>();
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].to_string(), "https://relay.example/");
    }

    #[test]
    fn every_protocol_has_a_distinct_tag() {
        for proto in Proto::ALL {
            assert_eq!(from_tag(tag(proto)), Some(proto));
        }
        assert_eq!(from_tag(0), None);
    }

    #[test]
    fn invite_addresses_yield_the_peer_and_a_direct_hint() {
        let peer = PeerId::random();
        let (got, direct) =
            parse_peer_addr(&format!("/ip4/192.168.1.5/udp/40000/quic-v1/p2p/{peer}")).unwrap();
        assert_eq!(got, peer);
        assert_eq!(direct, vec!["192.168.1.5:40000".parse().unwrap()]);
        let (_, none) = parse_peer_addr(&format!("/ip4/10.0.0.1/tcp/4001/p2p/{peer}")).unwrap();
        assert!(none.is_empty(), "a libp2p TCP port is no hint for Iroh");
    }
}
