use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "enochd", about = "ENOCHIAN daemon — P2P agent collaboration protocol")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Create a new Circle and generate its keypair + PSK
    Init(InitArgs),
    /// Start the ENOCHIAN daemon for a Circle
    Serve(ServeArgs),
    /// Join an existing Circle
    Enter(EnterArgs),
}

#[derive(Parser)]
pub struct InitArgs {
    /// Human-readable name for the Circle
    #[arg(long)]
    pub name: String,
}

#[derive(Parser)]
pub struct ServeArgs {
    /// Circle ID (UUID) to serve
    #[arg(long)]
    pub circle: String,

    /// TCP port to listen on
    #[arg(long, default_value = "9090")]
    pub port: u16,
}

#[derive(Parser)]
pub struct EnterArgs {
    /// Circle ID to join
    pub circle_id: String,

    /// Pre-shared key (hex) issued by the circle operator
    #[arg(long)]
    pub secret: String,

    /// Rendezvous server multiaddr for WAN peer discovery
    #[arg(long)]
    pub rendezvous: Option<String>,

    /// Directly dial a peer multiaddr, e.g. /ip4/192.168.1.10/tcp/9090
    /// Useful when mDNS is blocked (Windows Firewall, different subnets)
    #[arg(long)]
    pub peer: Option<String>,
}
