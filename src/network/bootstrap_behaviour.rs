use libp2p::{identify, kad, ping, relay, rendezvous, swarm::NetworkBehaviour};

/// Swarm behaviour for the public bootstrap server (enoxd --bootstrap).
/// Provides rendezvous (peer discovery) and circuit relay for circle members.
/// Does not join any circle — holds no PSK.
#[derive(NetworkBehaviour)]
#[behaviour(to_swarm = "BootstrapEvent")]
pub struct BootstrapBehaviour {
    pub rendezvous: rendezvous::server::Behaviour,
    pub relay: relay::Behaviour,
    pub identify: identify::Behaviour,
    pub ping: ping::Behaviour,
    pub kad: kad::Behaviour<kad::store::MemoryStore>,
}

#[derive(Debug)]
pub enum BootstrapEvent {
    Rendezvous(rendezvous::server::Event),
    Relay(relay::Event),
    Identify(identify::Event),
    Ping(ping::Event),
    Kad(kad::Event),
}

impl From<rendezvous::server::Event> for BootstrapEvent {
    fn from(e: rendezvous::server::Event) -> Self {
        Self::Rendezvous(e)
    }
}

impl From<relay::Event> for BootstrapEvent {
    fn from(e: relay::Event) -> Self {
        Self::Relay(e)
    }
}

impl From<identify::Event> for BootstrapEvent {
    fn from(e: identify::Event) -> Self {
        Self::Identify(e)
    }
}

impl From<ping::Event> for BootstrapEvent {
    fn from(e: ping::Event) -> Self {
        Self::Ping(e)
    }
}

impl From<kad::Event> for BootstrapEvent {
    fn from(e: kad::Event) -> Self {
        Self::Kad(e)
    }
}
