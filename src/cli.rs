use clap::{Parser, Subcommand};

// ── CLI ────────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name = "enox",
    about = "enoxian agent CLI — collaborate inside a Circle",
    version = env!("CARGO_PKG_VERSION")
)]
pub struct AgentCli {
    /// Output raw JSON (machine-readable)
    #[arg(long, global = true)]
    pub json: bool,

    /// Circle name or ID prefix to target (overrides ENOXIAN_CIRCLE)
    #[arg(long, global = true, env = "ENOXIAN_CIRCLE")]
    pub circle: Option<String>,

    /// Short-lived actor token from `enox register`; may appear after the command
    #[arg(long, global = true)]
    pub token: Option<String>,

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
    /// Register an external agent label on this device and issue a 1-hour token
    Register {
        /// Agent label used for task, chat, and lock attribution
        agent_id: String,
    },
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
    /// Start the Enoxian background service
    Start {
        /// Port to listen on
        #[arg(long)]
        port: Option<u16>,
    },
    /// Stop the running Enoxian background service
    Stop,
    /// Run the local daemon in the foreground (advanced)
    Daemon(DaemonArgs),
    /// Run a public rendezvous and circuit-relay server
    Bootstrap(BootstrapArgs),
    /// Install and manage login-time background startup
    Service(ServiceArgs),
    /// Update Enoxian or inspect the active update channel
    Update {
        /// Build from source instead of downloading a release binary.
        /// Use this during development.
        #[arg(long, conflicts_with = "status")]
        dev: bool,
        /// Path to the enoxian source directory (saved after first use)
        #[arg(long, requires = "dev")]
        src: Option<std::path::PathBuf>,
        /// Skip git pull (just rebuild)
        #[arg(long, requires = "dev")]
        no_pull: bool,
        /// Show channel, source, managed binary, version, and service state
        #[arg(long, conflicts_with_all = ["dev", "src", "no_pull"])]
        status: bool,
        /// Installer-only marker for a verified stable release
        #[arg(long, hide = true, conflicts_with_all = ["dev", "src", "no_pull", "status"])]
        record_stable: bool,
    },
    /// Complete a deferred self-update after the old executable exits
    #[command(name = "update-apply", hide = true)]
    UpdateApply(UpdateApplyArgs),
    /// Manage this device's identity (label, user linking)
    Identity(IdentityArgs),
    /// Review workspace change proposals (list, show, accept, reject, revert)
    Proposal(ProposalArgs),
    /// Run a configured agent in the workspace under a change session
    Agent(AgentRunArgs),
    /// Declare a local change session (claimed session mode)
    Session(SessionArgs),
}

#[derive(clap::Args)]
pub struct UpdateApplyArgs {
    #[arg(long)]
    pub source: std::path::PathBuf,
    #[arg(long)]
    pub target: std::path::PathBuf,
    #[arg(long)]
    pub service: bool,
    #[arg(long)]
    pub dev_source: std::path::PathBuf,
}

#[derive(clap::Args)]
pub struct AgentRunArgs {
    #[command(subcommand)]
    pub action: AgentAction,
}

#[derive(Subcommand)]
pub enum AgentAction {
    /// Launch an agent from `~/.enoxian/agents.toml` and run a task.
    /// enoxian owns the process, so its file changes become an attributed
    /// (managed-process) proposal.
    Run {
        /// Agent name as configured in agents.toml (e.g. "claude").
        agent: String,
        /// The task/prompt text passed to the agent.
        task: String,
    },
    /// List this device's configured agents and reaction policy.
    List,
    /// List managed adapter plugins and their install/health state.
    Plugins,
    /// Install or repair a pinned managed adapter and configure its @handle.
    Install {
        /// Plugin id, e.g. `codex-acp` or `claude`.
        plugin: String,
    },
    /// Add or replace an agent in agents.toml.
    ///
    /// Example:
    ///   enox agent add my-acp --driver acp -- /path/to/my-acp-adapter
    Add {
        /// Name to mention (e.g. "claude").
        name: String,
        /// Launch driver: "acp" (Agent Client Protocol) or "argv" (plain spawn).
        #[arg(long, default_value = "acp")]
        driver: String,
        /// Working directory relative to the workspace root.
        #[arg(long)]
        working_dir: Option<String>,
        /// The command and arguments to launch. Everything after `--`.
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
    /// Remove an agent from agents.toml.
    Remove { name: String },
    /// Set how this device reacts to @mentions: "push" (auto-run) or "pull"
    /// (do nothing). push lets a circle member's mention run a local process.
    Reaction {
        #[arg(value_parser = ["push", "pull"])]
        mode: String,
    },
}

#[derive(clap::Args)]
pub struct SessionArgs {
    #[command(subcommand)]
    pub action: SessionAction,
}

#[derive(Subcommand)]
pub enum SessionAction {
    /// Open a claimed session: changes to the workspace until `session finish`
    /// are attributed to `--actor` (user-declared confidence).
    Start {
        /// Who to attribute changes to (e.g. an agent or tool name).
        #[arg(long)]
        actor: String,
    },
    /// Close the open claimed session on this workspace.
    Finish,
}

#[derive(clap::Args)]
pub struct ProposalArgs {
    #[command(subcommand)]
    pub action: ProposalAction,
}

#[derive(Subcommand)]
pub enum ProposalAction {
    /// List proposals (newest first)
    List,
    /// Show a proposal's metadata and per-file diff
    Show {
        /// Proposal id (full or unambiguous prefix accepted by the daemon)
        id: String,
    },
    /// Accept a pending proposal (keep the changes)
    Accept { id: String },
    /// Reject a pending proposal (restore files to their pre-change state)
    Reject { id: String },
    /// Revert a previously accepted proposal
    Revert { id: String },
}

#[derive(clap::Args)]
pub struct IdentityArgs {
    #[command(subcommand)]
    pub action: IdentityAction,
}

#[derive(Subcommand)]
pub enum IdentityAction {
    /// Show this device's identity
    Show,
    /// Set the device label (shown in presence)
    SetLabel { label: String },
    /// Set a user handle (shown in presence, links all your devices visually)
    SetUser { handle: String },
    /// Create a new user identity and link this device to it.
    /// Prints a 24-word mnemonic — back it up to link other devices.
    CreateUser {
        /// Your chosen handle (e.g. "suzy")
        handle: String,
    },
    /// Link this device to an existing user via a BIP-39 mnemonic.
    LinkUser {
        /// Your user handle
        handle: String,
        /// The 24-word mnemonic (quote the whole phrase)
        mnemonic: String,
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

    /// Embed a relay multiaddr for WAN connectivity (e.g. /ip4/1.2.3.4/tcp/36522/p2p/<peer_id>)
    #[arg(long)]
    pub relay: Option<String>,

    /// Embed a rendezvous server multiaddr for automatic peer discovery
    /// (e.g. /ip4/1.2.3.4/udp/36521/quic-v1/p2p/<peer_id>)
    #[arg(long)]
    pub rendezvous: Option<String>,
}

#[derive(clap::Args, Clone)]
pub struct ServeArgs {
    /// Port to listen on
    #[arg(long, default_value = "36521")]
    pub port: u16,

    /// Bind the HTTP/WS API to all interfaces (0.0.0.0) instead of loopback.
    /// The API is a privileged control plane; only expose it on a LAN you trust,
    /// and prefer tunnelling loopback over exposing it directly. Requires the
    /// API token for every request regardless.
    #[arg(long)]
    pub bind_lan: bool,

    /// Explicit bind address (overrides --bind-lan). E.g. 127.0.0.1 or 0.0.0.0.
    #[arg(long)]
    pub bind: Option<std::net::IpAddr>,
}

#[derive(clap::Args)]
pub struct DaemonArgs {
    #[command(subcommand)]
    pub action: DaemonAction,
}

#[derive(Subcommand)]
pub enum DaemonAction {
    /// Run all enabled Circles and the local API in the foreground
    Run(ServeArgs),
}

#[derive(clap::Args)]
pub struct BootstrapArgs {
    #[command(subcommand)]
    pub action: BootstrapAction,
}

#[derive(Subcommand)]
pub enum BootstrapAction {
    /// Serve rendezvous discovery and circuit relay traffic
    Serve(BootstrapServeArgs),
}

#[derive(clap::Args)]
pub struct BootstrapServeArgs {
    /// QUIC rendezvous and HTTP status port
    #[arg(long, default_value = "36521")]
    pub port: u16,

    /// TCP circuit relay port (defaults to --port + 1)
    #[arg(long)]
    pub relay_port: Option<u16>,

    /// Public DNS hostname advertised by the bootstrap server
    #[arg(long, env = "ENOXIAN_ADVERTISE_HOST")]
    pub advertise_host: Option<String>,
}

#[derive(clap::Args)]
pub struct ServiceArgs {
    #[command(subcommand)]
    pub action: ServiceAction,
}

#[derive(Subcommand)]
pub enum ServiceAction {
    /// Install, enable, and start the per-user login service
    Install {
        /// Local API port saved in the service definition
        #[arg(long, default_value = "36521")]
        port: u16,
        /// Bind the privileged API beyond loopback
        #[arg(long)]
        bind_lan: bool,
        /// Explicit bind address (overrides --bind-lan)
        #[arg(long)]
        bind: Option<std::net::IpAddr>,
        /// Replace an existing service definition
        #[arg(long)]
        force: bool,
    },
    /// Show service installation, process, and API health
    Status,
    /// Start the installed service
    Start,
    /// Stop the service without uninstalling it
    Stop,
    /// Restart the installed service
    Restart,
    /// Follow service logs
    Logs,
    /// Stop and remove the managed service
    Uninstall,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_token_is_accepted_after_subcommand() {
        let cli = AgentCli::try_parse_from(["enox", "claim", "task-1", "--token", "secret"])
            .expect("global token should parse after the subcommand");
        assert_eq!(cli.token.as_deref(), Some("secret"));
        assert!(matches!(cli.command, AgentCommands::Claim { .. }));
    }

    #[test]
    fn register_accepts_agent_label() {
        let cli = AgentCli::try_parse_from(["enox", "register", "hermes"])
            .expect("register command should parse");
        assert!(matches!(
            cli.command,
            AgentCommands::Register { agent_id } if agent_id == "hermes"
        ));
    }
}
