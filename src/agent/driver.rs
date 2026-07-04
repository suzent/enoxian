//! Local execution layer: run a permitted agent in the workspace.
//!
//! Two drivers behind one entry point (`launch`):
//!
//! - **argv** — spawn the command with `{{task}}` substituted and wait. The
//!   agent touches the workspace directly; the ambient proposal engine captures
//!   the result. Universal fallback: the agent needs to know nothing about
//!   enoxian.
//! - **acp** — drive the agent over the Agent Client Protocol (`super::acp`).
//!   Gives a real prompt-turn lifecycle and, when the agent uses client fs
//!   methods, mediated per-write access.
//!
//! Either way the run happens inside a `LocalChangeSession`, so the proposal
//! the engine emits can be attributed to the agent rather than left ambient.

use super::acp::{agent_message_text, AcpSession, ClientHooks, PermissionDecision};
use super::config::{AgentCommand, Driver};
use crate::proposal::session::{LocalChangeSession, SessionMode};
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Where the run was initiated from — decides the session mode and, downstream,
/// the acceptance policy origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Initiator {
    /// A local chat mention this device chose to react to (push policy), or a
    /// local `enox agent run`.
    Local,
    /// Another circle member's mention this device chose to react to.
    RemoteMember,
}

/// Outcome of a launch.
#[derive(Debug, Clone)]
pub struct LaunchOutcome {
    pub session_id: String,
    pub mode: SessionMode,
    /// For ACP runs, the turn's stop reason; for argv, the process exit status.
    pub detail: String,
    /// The agent's streamed text reply, if any (ACP driver only). The caller
    /// posts this to chat so the mention reads like a conversation.
    pub reply: Option<String>,
}

/// Permission hook that defers every ACP permission request to a fixed policy
/// decision. The daemon-level acceptance policy still governs whether the
/// resulting proposal auto-accepts; this only gates the agent's in-turn actions.
struct PolicyHooks {
    allow: bool,
    /// Accumulates the agent's streamed message text so the reply can be posted
    /// to chat after the turn. `&self`-only trait, hence the shared mutable cell.
    reply: Arc<Mutex<String>>,
}

impl ClientHooks for PolicyHooks {
    fn on_permission(&self, tool: &Value) -> PermissionDecision {
        tracing::info!("[agent] permission requested: {}", compact(tool));
        if self.allow {
            PermissionDecision::Allow
        } else {
            PermissionDecision::Deny
        }
    }
    fn on_update(&self, update: &Value) {
        if let Some(text) = agent_message_text(update) {
            if let Ok(mut buf) = self.reply.lock() {
                buf.push_str(&text);
            }
        } else {
            tracing::debug!("[agent] session update: {}", compact(update));
        }
    }
}

/// Launch a permitted agent against `workspace`, running the given task under a
/// change session. Returns once the agent finishes its work.
pub async fn launch(
    agent_name: &str,
    cmd: &AgentCommand,
    task: &str,
    workspace: &Path,
    base_snapshot: &str,
    circle_id: &str,
    initiator: Initiator,
) -> Result<LaunchOutcome> {
    let mode = match initiator {
        // A managed run enoxian owns the process tree for → verified process.
        Initiator::Local | Initiator::RemoteMember => SessionMode::ManagedProcess,
    };
    let mut session = LocalChangeSession::start(circle_id.to_string(), base_snapshot.to_string(), mode);
    session.requested_agent = Some(agent_name.to_string());
    session.actor_id = Some(agent_name.to_string());
    tracing::info!(
        "[agent] launching `{agent_name}` ({:?}) session={} task={:?}",
        cmd.driver, session.session_id, task
    );

    let run_dir = working_dir(workspace, cmd.working_dir.as_deref());

    let (detail, reply) = match cmd.driver {
        Driver::Argv => (run_argv(cmd, task, &run_dir).await?, None),
        Driver::Acp => run_acp(cmd, initiator, task, &run_dir).await?,
    };

    session.finish();
    Ok(LaunchOutcome {
        session_id: session.session_id,
        mode,
        detail,
        reply,
    })
}

async fn run_argv(cmd: &AgentCommand, task: &str, run_dir: &Path) -> Result<String> {
    let rendered = cmd.render(task);
    let (program, args) = rendered
        .split_first()
        .context("empty agent command")?;
    let status = super::spawn::command(program, args)
        .current_dir(run_dir)
        .status()
        .await
        .with_context(|| format!("failed to spawn agent `{program}`"))?;
    Ok(format!("exit {status}"))
}

async fn run_acp(
    cmd: &AgentCommand,
    _task_initiator: Initiator,
    task: &str,
    run_dir: &Path,
) -> Result<(String, Option<String>)> {
    // Always allow the agent to act *within the workspace* — that is its job,
    // and enoxian captures whatever it writes as a proposal. Deny-at-tool-call
    // would just make a mentioned agent unable to do anything. The local-vs-
    // remote safety distinction lives one layer up, in the acceptance policy
    // (auto-accept vs pending-review of the resulting proposal), not here.
    let reply = Arc::new(Mutex::new(String::new()));
    let hooks = PolicyHooks { allow: true, reply: reply.clone() };

    let mut acp = AcpSession::start(&cmd.command, run_dir, hooks)
        .await
        .context("ACP handshake failed")?;
    let result = acp.prompt(task).await;
    acp.shutdown().await;
    let turn = result.context("ACP prompt turn failed")?;

    let text = reply.lock().ok().map(|b| b.trim().to_string()).filter(|s| !s.is_empty());
    Ok((format!("stop_reason={}", turn.stop_reason), text))
}

fn working_dir(workspace: &Path, rel: Option<&str>) -> PathBuf {
    match rel {
        Some(r) if !r.is_empty() => workspace.join(r),
        _ => workspace.to_path_buf(),
    }
}

fn compact(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}
