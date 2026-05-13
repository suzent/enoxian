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
    /// Join a Circle using an invite URI or Circle ID + secret
    Enter(EnterArgs),
    /// Generate a new invite link for an existing Circle
    Invite(InviteArgs),
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

    /// How long the initial invite link is valid (e.g. 7d, 24h)
    #[arg(long, default_value = "7d")]
    pub ttl: String,
}

#[derive(Parser)]
pub struct EnterArgs {
    /// enochian:// invite URI  — OR —  a Circle ID (requires --secret)
    pub target: String,

    /// Pre-shared key (hex) — required when target is a raw Circle ID
    #[arg(long)]
    pub secret: Option<String>,

    /// Directly dial a peer multiaddr (overrides any peer embedded in the invite)
    #[arg(long)]
    pub peer: Option<String>,

    /// Rendezvous server multiaddr for WAN
    #[arg(long)]
    pub rendezvous: Option<String>,
}

#[derive(Parser)]
pub struct InviteArgs {
    /// Circle ID to generate an invite for
    pub circle_id: String,

    /// How long the invite is valid (e.g. 7d, 24h)
    #[arg(long, default_value = "7d")]
    pub ttl: String,

    /// Embed a peer multiaddr so invitees can connect without mDNS (e.g. /ip4/1.2.3.4/tcp/9091)
    #[arg(long)]
    pub peer: Option<String>,
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
