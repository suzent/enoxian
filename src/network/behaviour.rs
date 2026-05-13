use libp2p::{identify, kad, mdns, ping, rendezvous, swarm::NetworkBehaviour};
use libp2p_stream as stream;

#[derive(NetworkBehaviour)]
#[behaviour(to_swarm = "EnochEvent")]
pub struct EnochBehaviour {
    pub mdns: mdns::tokio::Behaviour,
    pub kad: kad::Behaviour<kad::store::MemoryStore>,
    pub identify: identify::Behaviour,
    pub ping: ping::Behaviour,
    pub rendezvous: rendezvous::client::Behaviour,
    pub stream: stream::Behaviour,
}

#[derive(Debug)]
pub enum EnochEvent {
    Mdns(mdns::Event),
    Kad(kad::Event),
    Identify(identify::Event),
    Ping(ping::Event),
    Rendezvous(rendezvous::client::Event),
    /// stream::Behaviour emits no user events (ToSwarm = ())
    Stream,
}

impl From<mdns::Event> for EnochEvent {
    fn from(e: mdns::Event) -> Self { Self::Mdns(e) }
}

impl From<kad::Event> for EnochEvent {
    fn from(e: kad::Event) -> Self { Self::Kad(e) }
}

impl From<identify::Event> for EnochEvent {
    fn from(e: identify::Event) -> Self { Self::Identify(e) }
}

impl From<ping::Event> for EnochEvent {
    fn from(e: ping::Event) -> Self { Self::Ping(e) }
}

impl From<rendezvous::client::Event> for EnochEvent {
    fn from(e: rendezvous::client::Event) -> Self { Self::Rendezvous(e) }
}

impl From<()> for EnochEvent {
    fn from(_: ()) -> Self { Self::Stream }
}
