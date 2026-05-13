use clap::{Parser, Subcommand};

// ── Daemon CLI ─────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "enochd", about = "ENOCHIAN daemon — runs the P2P sync node")]
pub struct DaemonCli {
    #[command(subcommand)]
    pub command: DaemonCommands,
}

#[derive(Subcommand)]
pub enum DaemonCommands {
    /// Start the ENOCHIAN daemon for a Circle
    Serve(ServeArgs),
}

// ── Agent CLI ──────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "enoch", about = "ENOCHIAN agent CLI — collaborate inside a Circle")]
pub struct AgentCli {
    /// Output raw JSON (machine-readable)
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: AgentCommands,
}

#[derive(Subcommand)]
pub enum AgentCommands {
    /// Create a new Circle (generates keypair + PSK)
    Init(InitArgs),
    /// Join an existing Circle
    Enter(EnterArgs),
    /// Show Circle overview
    Status,
    /// Show agent presence
    Who,
    /// List tasks
    Tasks {
        /// Filter by status (open | claimed | done)
        #[arg(long)]
        status: Option<String>,
    },
    /// Claim a task
    Claim { task_id: String },
    /// Mark a task as done
    Done { task_id: String },
    /// Acquire an explicit file lock
    Bind { path: String },
    /// Release a file lock
    Release { path: String },
    /// Stream live Circle events
    Watch,
}

// ── Shared arg structs ─────────────────────────────────────────────────────

#[derive(Parser)]
pub struct InitArgs {
    /// Human-readable name for the Circle
    #[arg(long)]
    pub name: String,
}

#[derive(Parser)]
pub struct ServeArgs {
    /// Circle ID (UUID)
    #[arg(long)]
    pub circle: String,

    /// Port to listen on
    #[arg(long, default_value = "9090")]
    pub port: u16,

    /// Directory to sync (defaults to ~/.enochian/circles/<id>/files)
    #[arg(long)]
    pub sync_dir: Option<std::path::PathBuf>,
}

#[derive(Parser)]
pub struct EnterArgs {
    /// Circle ID to join
    pub circle_id: String,

    /// Pre-shared key (hex)
    #[arg(long)]
    pub secret: String,

    /// Rendezvous server multiaddr for WAN
    #[arg(long)]
    pub rendezvous: Option<String>,

    /// Directly dial a peer multiaddr (bypasses mDNS)
    #[arg(long)]
    pub peer: Option<String>,
}
