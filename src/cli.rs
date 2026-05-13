use clap::{Parser, Subcommand};

// ── Daemon CLI ─────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "enochd", about = "ENOCHIAN daemon — serves all known Circles over HTTP/P2P")]
pub struct DaemonCli {
    /// Port to listen on
    #[arg(long, default_value = "9090")]
    pub port: u16,
}

// ── Agent CLI ──────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "enoch", about = "ENOCHIAN agent CLI — collaborate inside a Circle")]
pub struct AgentCli {
    /// Output raw JSON (machine-readable)
    #[arg(long, global = true)]
    pub json: bool,

    /// Circle name or ID prefix to target (overrides ENOCHIAN_CIRCLE)
    #[arg(long, global = true, env = "ENOCHIAN_CIRCLE")]
    pub circle: Option<String>,

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
    /// List all known Circles (local) or active ones (daemon)
    Circles,
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
    /// Circle name, name prefix, or UUID prefix to generate an invite for
    pub circle: String,

    /// How long the invite is valid (e.g. 7d, 24h)
    #[arg(long, default_value = "7d")]
    pub ttl: String,

    /// Embed a peer multiaddr so invitees can connect without mDNS (e.g. /ip4/1.2.3.4/tcp/9091)
    #[arg(long)]
    pub peer: Option<String>,
}

#[derive(Parser)]
pub struct ServeArgs {
    /// Port to listen on
    #[arg(long, default_value = "9090")]
    pub port: u16,
}
