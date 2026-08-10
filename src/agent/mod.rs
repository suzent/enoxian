//! Local agent execution: the layer that runs agents on *this* device.
//!
//! A chat mention is intent, not a command (see `docs/plan/agent-workspaces.md`
//! → Two-Layer Split). This module owns the local side of that boundary:
//!
//! - [`config`] — the daemon-local allowlist, per-agent driver, and this
//!   device's push/pull reaction policy. Never synced.
//! - [`reaction`] — the loop that watches chat mentions and, under a push
//!   policy, launches a permitted agent.
//! - [`driver`] — the execution layer: run an agent (argv or ACP) inside a
//!   `LocalChangeSession` so its file changes become an attributed proposal.
//! - [`acp`] — a minimal Agent Client Protocol client (enoxian is the client,
//!   the coding agent is the agent).

pub mod acp;
pub mod config;
pub mod context;
pub mod driver;
pub mod handled;
pub mod memory;
pub mod mention;
pub mod plugin;
pub mod probe;
pub mod reaction;
pub mod spawn;
