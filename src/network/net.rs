//! What the protocols need from a transport, and nothing more.
//!
//! A protocol is handed a peer and one bidirectional byte stream, and may ask
//! for a new stream to a peer. Which transport carries it, how peers are found
//! and how connections are kept up is the transport's business. Keeping that
//! out of the protocol modules is what lets the transport be replaced without
//! touching sync, proposals, events, bootstrap or admin handover.
//!
//! Peers are still named by [`PeerId`]: it is the Circle's identity for a
//! member (signatures, the member list, MLS credentials), not a transport
//! detail, and stays when the transport changes.

use anyhow::Result;
use futures::future::BoxFuture;
use libp2p_identity::PeerId;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::state::AppState;

/// A bidirectional byte stream to one peer.
pub trait Duplex: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Duplex for T {}

/// The stream a protocol runs over.
pub type PeerStream = Box<dyn Duplex>;

/// The protocols a Circle speaks, one stream each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Proto {
    /// Plaintext MLS membership delivery, for peers without content keys yet.
    MlsBootstrap,
    /// A leaving admin handing its key to a successor.
    AdminHandover,
    /// Encrypted CRDT sync.
    Sync,
    /// Encrypted proposal history.
    Proposals,
    /// Encrypted workspace events.
    Events,
}

impl Proto {
    /// Every protocol a Circle accepts.
    pub const ALL: [Proto; 5] = [
        Proto::MlsBootstrap,
        Proto::AdminHandover,
        Proto::Sync,
        Proto::Proposals,
        Proto::Events,
    ];

    /// What the dialing side of a new connection opens, in this order.
    /// Admin handover is opened on demand instead.
    pub const ON_CONNECT: [Proto; 4] = [
        Proto::MlsBootstrap,
        Proto::Sync,
        Proto::Proposals,
        Proto::Events,
    ];

    /// Log prefix.
    pub fn name(self) -> &'static str {
        match self {
            Proto::MlsBootstrap => "mls-bootstrap",
            Proto::AdminHandover => "admin-handover",
            Proto::Sync => "sync",
            Proto::Proposals => "proposal-sync",
            Proto::Events => "event-sync",
        }
    }
}

/// Opening streams to peers, for protocols that start one on their own.
pub trait NetHandle: Send + Sync {
    fn open(&self, peer: PeerId, proto: Proto) -> BoxFuture<'static, Result<PeerStream>>;
}

/// Run `proto` over `stream`. `initiator` is the side that opened the stream.
pub async fn serve(
    proto: Proto,
    peer: PeerId,
    stream: PeerStream,
    state: AppState,
    initiator: bool,
) {
    use super::{admin_handover, event_sync, mls_bootstrap, proposal_sync, sync};
    match proto {
        Proto::MlsBootstrap => mls_bootstrap::run(peer, stream, state, initiator).await,
        Proto::AdminHandover => admin_handover::accept(peer, stream, state).await,
        Proto::Sync => sync::run_sync(peer, stream, state, initiator).await,
        Proto::Proposals => proposal_sync::run(peer, stream, state, initiator).await,
        Proto::Events => event_sync::run(peer, stream, state, initiator).await,
    }
}
