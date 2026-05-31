use clap::{Parser, Subcommand};

// ── Daemon CLI ─────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "enoxd", about = "enoxian daemon — serves all known Circles over HTTP/P2P")]
pub struct DaemonCli {
    /// Port to listen on
    #[arg(long, default_value = "36521")]
    pub port: u16,

    /// Run as a public bootstrap server (rendezvous + relay, no circles).
    /// Generates a stable keypair at ~/.enoxian/bootstrap.key on first run.
    /// Circle members connect via QUIC — no PSK required.
    #[arg(long)]
    pub bootstrap: bool,
}

// ── Agent CLI ──────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "enox", about = "enoxian agent CLI — collaborate inside a Circle")]
pub struct AgentCli {
    /// Output raw JSON (machine-readable)
    #[arg(long, global = true)]
    pub json: bool,

    /// Circle name or ID prefix to target (overrides enoxian_CIRCLE)
    #[arg(long, global = true, env = "enoxian_CIRCLE")]
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
    /// Create a new task
    TaskCreate {
        /// Task title
        title: String,
        /// Optional task description
        #[arg(long)]
        description: Option<String>,
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
    /// Disable a Circle (stops it and prevents auto-start)
    Disable,
    /// Enable a disabled Circle (allows auto-start)
    Enable,
    /// Leave a Circle permanently (removes local config)
    Leave {
        /// Skip confirmation prompt
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Manage Circle members
    Member(MemberArgs),
    /// Show recent chat messages
    Chat {
        /// Stream new messages as they arrive
        #[arg(long, short = 'f')]
        follow: bool,
        /// Only show messages after this Unix timestamp
        #[arg(long)]
        since: Option<i64>,
    },
    /// Post a chat message
    Say {
        /// Message text (use @agent_id to mention an agent)
        text: String,
    },
    /// Open the Circle UI in the default browser
    Open,
    /// Start the enoxd daemon in the background
    Start {
        /// Port to listen on
        #[arg(long, default_value = "36521")]
        port: u16,
    },
    /// Stop the running enoxd daemon
    Stop,
    /// Update enox and enoxd to the latest version
    Update {
        /// Build from source instead of downloading a release binary.
        /// Use this during development.
        #[arg(long)]
        dev: bool,
        /// Path to the enoxian source directory (saved after first use)
        #[arg(long)]
        src: Option<std::path::PathBuf>,
        /// Skip git pull (just rebuild)
        #[arg(long)]
        no_pull: bool,
    },
}

#[derive(Parser)]
pub struct MemberArgs {
    #[command(subcommand)]
    pub action: MemberAction,
}

#[derive(Subcommand)]
pub enum MemberAction {
    /// List members
    List,
    /// Add a member (auto-signs with admin.key if present)
    Add {
        peer_id: String,
        /// Role: member (default) or admin
        #[arg(long, default_value = "member")]
        role: String,
        /// Human owner of this peer, e.g. "alice". Defaults to agent_id reported by the peer.
        #[arg(long)]
        owner: Option<String>,
        /// Agent identifier for this peer, e.g. "alice-suzent". Defaults to owner.
        #[arg(long)]
        agent_id: Option<String>,
    },
    /// Remove a member (auto-signs with admin.key if present)
    Remove { peer_id: String },
    /// Promote a member to admin (auto-signs with admin.key if present)
    Promote { peer_id: String },
    /// List pending join requests
    Pending,
    /// Approve a pending member (auto-signs with admin.key)
    Approve {
        peer_id: String,
        #[arg(long, default_value = "member")]
        role: String,
        /// Override the claimed owner name
        #[arg(long)]
        owner: Option<String>,
    },
    /// Reject a pending member
    Reject { peer_id: String },
    /// Remove all peers owned by a given owner
    RemoveByOwner { owner: String },
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

    /// Workspace directory (default: ~/enoxian/<name>)
    #[arg(long)]
    pub dir: Option<std::path::PathBuf>,

    #[arg(long)]
    pub owner: Option<String>,

    #[arg(long, default_value = "auto")]
    pub join_policy: String,
}

#[derive(Parser)]
pub struct EnterArgs {
    /// enoxian:// invite URI  — OR —  a Circle ID (requires --secret)
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

    /// Workspace directory (default: ~/enoxian/<circle-name>)
    #[arg(long)]
    pub dir: Option<std::path::PathBuf>,

    #[arg(long)]
    pub owner: Option<String>,

    /// Skip the 10-second connectivity verification step.
    /// Set automatically when called from the daemon API — the daemon's P2P
    /// swarm handles connectivity; blocking the HTTP handler is not desirable.
    #[arg(skip)]
    pub no_verify: bool,
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

    /// Embed a relay multiaddr for WAN connectivity (e.g. /ip4/1.2.3.4/tcp/36521/p2p/<peer_id>)
    #[arg(long)]
    pub relay: Option<String>,

    /// Embed a rendezvous server multiaddr for automatic peer discovery
    /// (e.g. /ip4/1.2.3.4/udp/36521/quic-v1/p2p/<peer_id>)
    #[arg(long)]
    pub rendezvous: Option<String>,
}

#[derive(Parser)]
pub struct ServeArgs {
    /// Port to listen on
    #[arg(long, default_value = "36521")]
    pub port: u16,
}
