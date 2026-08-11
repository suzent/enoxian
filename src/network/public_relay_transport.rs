use std::{
    collections::HashSet,
    pin::Pin,
    sync::{Arc, RwLock},
    task::{Context, Poll},
};

use libp2p::{
    core::transport::{DialOpts, ListenerId, TransportError, TransportEvent},
    multiaddr::Protocol,
    Multiaddr, PeerId, Transport,
};

/// Restricts a public, non-pnet transport to known relay servers only.
///
/// Circle peer TCP still goes through the pnet transport. This wrapper exists so
/// relay reservation dials can reach public infrastructure that does not know
/// the circle PSK, without opening a general-purpose no-PSK TCP path.
pub struct PublicRelayTransport<T> {
    inner: T,
    relay_peer_ids: Arc<RwLock<HashSet<PeerId>>>,
}

impl<T> PublicRelayTransport<T> {
    pub fn new(inner: T, relay_peer_ids: Arc<RwLock<HashSet<PeerId>>>) -> Self {
        Self {
            inner,
            relay_peer_ids,
        }
    }
}

impl<T> Transport for PublicRelayTransport<T>
where
    T: Transport + Unpin,
{
    type Output = T::Output;
    type Error = T::Error;
    type ListenerUpgrade = T::ListenerUpgrade;
    type Dial = T::Dial;

    fn listen_on(
        &mut self,
        _id: ListenerId,
        addr: Multiaddr,
    ) -> Result<(), TransportError<Self::Error>> {
        Err(TransportError::MultiaddrNotSupported(addr))
    }

    fn remove_listener(&mut self, id: ListenerId) -> bool {
        self.inner.remove_listener(id)
    }

    fn dial(
        &mut self,
        addr: Multiaddr,
        opts: DialOpts,
    ) -> Result<Self::Dial, TransportError<Self::Error>> {
        let relay_peer_ids = self.relay_peer_ids.read().unwrap();
        if !is_allowed_public_relay_addr(&addr, &relay_peer_ids) {
            return Err(TransportError::MultiaddrNotSupported(addr));
        }
        self.inner.dial(addr, opts)
    }

    fn poll(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<TransportEvent<Self::ListenerUpgrade, Self::Error>> {
        Pin::new(&mut self.inner).poll(cx)
    }
}

pub fn relay_peer_ids_from_addrs<'a>(
    addrs: impl IntoIterator<Item = &'a String>,
) -> HashSet<PeerId> {
    addrs
        .into_iter()
        .filter_map(|addr| addr.parse::<Multiaddr>().ok())
        .filter_map(|addr| relay_peer_id(&addr))
        .collect()
}

pub fn relay_peer_id(addr: &Multiaddr) -> Option<PeerId> {
    addr.iter().find_map(|protocol| match protocol {
        Protocol::P2p(peer_id) => Some(peer_id),
        Protocol::P2pCircuit => None,
        _ => None,
    })
}

pub fn is_relayed_addr(addr: &Multiaddr) -> bool {
    addr.iter()
        .any(|protocol| matches!(protocol, Protocol::P2pCircuit))
}

fn is_allowed_public_relay_addr(addr: &Multiaddr, relay_peer_ids: &HashSet<PeerId>) -> bool {
    if relay_peer_ids.is_empty() {
        return false;
    }

    let mut has_tcp = false;
    let mut peer_id = None;

    for protocol in addr.iter() {
        match protocol {
            Protocol::Tcp(_) => has_tcp = true,
            Protocol::P2p(id) => peer_id = Some(id),
            Protocol::P2pCircuit => return false,
            _ => {}
        }
    }

    has_tcp && peer_id.is_some_and(|id| relay_peer_ids.contains(&id))
}
