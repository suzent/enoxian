//! The libp2p transport behind [`NetHandle`]: protocol ids, opening and
//! accepting streams, and naming the route a connection took.

use anyhow::Result;
use futures::{future::BoxFuture, Stream, StreamExt};
use libp2p::{multiaddr::Protocol, Multiaddr, PeerId, StreamProtocol};
use tokio_util::compat::FuturesAsyncReadCompatExt;

use super::net::{NetHandle, PeerStream, Proto};
use crate::state::{classify_ip, ConnectionKind};

/// The libp2p protocol id each protocol is negotiated under.
pub fn protocol(proto: Proto) -> StreamProtocol {
    StreamProtocol::new(match proto {
        Proto::MlsBootstrap => "/enoxian/mls-bootstrap/1.0.0",
        Proto::AdminHandover => "/enoxian/admin-handover/1.0.0",
        Proto::Sync => "/enoxian/sync/2.0.0",
        Proto::Proposals => "/enoxian/proposals/2.0.0",
        Proto::Events => "/enoxian/events/2.0.0",
    })
}

/// Opens streams through the swarm's stream behaviour.
pub struct Libp2pNet {
    control: libp2p_stream::Control,
}

impl Libp2pNet {
    pub fn new(control: libp2p_stream::Control) -> Self {
        Self { control }
    }
}

impl NetHandle for Libp2pNet {
    fn open(&self, peer: PeerId, proto: Proto) -> BoxFuture<'static, Result<PeerStream>> {
        let mut control = self.control.clone();
        Box::pin(async move {
            let stream = control
                .open_stream(peer, protocol(proto))
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(Box::new(stream.compat()) as PeerStream)
        })
    }
}

/// Incoming streams for `proto`, as the protocols take them.
pub fn accept(
    control: &mut libp2p_stream::Control,
    proto: Proto,
) -> Result<impl Stream<Item = (PeerId, PeerStream)>> {
    let incoming = control
        .accept(protocol(proto))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(incoming.map(|(peer, stream)| (peer, Box::new(stream.compat()) as PeerStream)))
}

/// The route a connection to `address` takes.
pub fn classify_address(address: &Multiaddr) -> ConnectionKind {
    if address
        .iter()
        .any(|protocol| matches!(protocol, Protocol::P2pCircuit))
    {
        return ConnectionKind::Relay;
    }
    for protocol in address.iter() {
        match protocol {
            Protocol::Ip4(ip) => return classify_ip(ip.into()),
            Protocol::Ip6(ip) => return classify_ip(ip.into()),
            _ => {}
        }
    }
    // A direct DNS address is externally routable unless the circuit marker
    // above identifies it as a relayed connection.
    ConnectionKind::Public
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These ids are on the wire: a device running an older release only
    /// talks to us if they stay exactly as they were.
    #[test]
    fn protocol_ids_are_unchanged() {
        let ids: Vec<String> = Proto::ALL
            .iter()
            .map(|proto| protocol(*proto).to_string())
            .collect();
        assert_eq!(
            ids,
            [
                "/enoxian/mls-bootstrap/1.0.0",
                "/enoxian/admin-handover/1.0.0",
                "/enoxian/sync/2.0.0",
                "/enoxian/proposals/2.0.0",
                "/enoxian/events/2.0.0",
            ]
        );
    }
}
