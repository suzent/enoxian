use libp2p::{
    dcutr, identify, kad, mdns, ping, relay, rendezvous,
    swarm::{behaviour::toggle::Toggle, NetworkBehaviour},
};
use libp2p_stream as stream;

#[derive(NetworkBehaviour)]
#[behaviour(to_swarm = "EnochEvent")]
pub struct EnochBehaviour {
    /// mDNS LAN auto-discovery. Optional (`Toggle`) and disabled by default:
    /// circle membership is invitation-based (bootstrap/relay/rendezvous addrs
    /// carried in the invite), so mDNS is only a same-LAN optimization — and
    /// libp2p-mdns 0.48's tokio timer livelocks on hosts with many virtual
    /// interfaces (Docker/VM/Tailscale), spinning CPU while doing zero work.
    pub mdns: Toggle<mdns::tokio::Behaviour>,
    pub kad: kad::Behaviour<kad::store::MemoryStore>,
    pub identify: identify::Behaviour,
    pub ping: ping::Behaviour,
    pub rendezvous: rendezvous::client::Behaviour,
    pub relay_client: relay::client::Behaviour,
    pub relay: relay::Behaviour,
    /// Direct connection upgrade through relay. Disabled in force-relay mode.
    pub dcutr: Toggle<dcutr::Behaviour>,
    pub stream: stream::Behaviour,
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum EnochEvent {
    Mdns(mdns::Event),
    Kad(kad::Event),
    Identify(identify::Event),
    Ping(ping::Event),
    Rendezvous(rendezvous::client::Event),
    RelayClient(relay::client::Event),
    Relay(relay::Event),
    Dcutr(dcutr::Event),
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

impl From<relay::client::Event> for EnochEvent {
    fn from(e: relay::client::Event) -> Self { Self::RelayClient(e) }
}

impl From<relay::Event> for EnochEvent {
    fn from(e: relay::Event) -> Self { Self::Relay(e) }
}

impl From<dcutr::Event> for EnochEvent {
    fn from(e: dcutr::Event) -> Self { Self::Dcutr(e) }
}

impl From<()> for EnochEvent {
    fn from(_: ()) -> Self { Self::Stream }
}
