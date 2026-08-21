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
//! Either way the run happens inside a `LocalChangeSession`. Session metadata
//! is retained for future attribution work; today the ambient engine records
//! direct filesystem writes as accepted, revertible history.

use super::acp::{agent_message_text, AcpSession, ClientHooks, PermissionDecision};
use super::config::{AgentCommand, Driver};
use crate::proposal::session::{LocalChangeSession, SessionMode};
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// The ACP session id after this run (ACP driver only). Persist it so the
    /// next mention of this agent can resume the conversation.
    pub acp_session_id: Option<String>,
}

/// Permission hook that defers every ACP permission request to a fixed policy
/// decision. This gates the agent's in-turn actions; filesystem changes it
/// makes are recorded afterward as accepted, revertible proposal history.
/// Segments the streamed assistant output into discrete messages. ACP streams
/// one assistant message as many `agent_message_chunk`s; a *different* kind of
/// update (a tool call, a user-message echo, a resumed-history user turn) marks
/// the boundary between messages. We keep each completed message separately so
/// the final reply is the agent's *last* message — its actual answer — not a
/// concatenation of every "let me look…" preamble and any replayed history.
#[derive(Default)]
struct ReplyBuf {
    /// Completed messages, in order.
    messages: Vec<String>,
    /// The message currently being streamed.
    current: String,
}

impl ReplyBuf {
    fn push_chunk(&mut self, text: &str) {
        self.current.push_str(text);
    }
    /// A non-agent-message update arrived — close the current message.
    fn boundary(&mut self) {
        let done = std::mem::take(&mut self.current);
        if !done.trim().is_empty() {
            self.messages.push(done);
        }
    }
    /// The reply to post: the last non-empty message (the agent's final answer).
    fn into_reply(mut self) -> Option<String> {
        self.boundary(); // flush any trailing in-progress message
        self.messages
            .into_iter()
            .rev()
            .find(|m| !m.trim().is_empty())
            .map(|m| m.trim().to_string())
    }
}

struct PolicyHooks {
    allow: bool,
    /// Segmented capture of the agent's streamed messages. `&self`-only trait,
    /// hence the shared mutable cell.
    reply: Arc<Mutex<ReplyBuf>>,
    /// When false, streamed text is ignored (during `session/load` history
    /// replay, before the current prompt turn).
    capturing: Arc<AtomicBool>,
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
        if !self.capturing.load(Ordering::Relaxed) {
            return;
        }
        if let Some(text) = agent_message_text(update) {
            if let Ok(mut buf) = self.reply.lock() {
                buf.push_chunk(&text);
            }
        } else {
            // Any non-message update ends the current assistant message.
            if let Ok(mut buf) = self.reply.lock() {
                buf.boundary();
            }
            tracing::debug!(
                "[agent] session update: {:?}",
                crate::agent::acp::update_kind(update)
            );
        }
    }
}

/// One agent run request.
pub struct LaunchRequest<'a> {
    pub agent_name: &'a str,
    pub cmd: &'a AgentCommand,
    /// The full prompt handed to the agent (task + any injected world context).
    pub task: &'a str,
    pub workspace: &'a Path,
    pub base_snapshot: &'a str,
    pub circle_id: &'a str,
    pub initiator: Initiator,
    /// Prior ACP session id to resume, if one is remembered for this agent.
    pub resume: Option<&'a str>,
}

/// Launch a permitted agent, running the given task under a change session.
/// Returns once the agent finishes its work.
pub async fn launch(req: LaunchRequest<'_>) -> Result<LaunchOutcome> {
    let mode = match req.initiator {
        // A managed run enoxian owns the process tree for → verified process.
        Initiator::Local | Initiator::RemoteMember => SessionMode::ManagedProcess,
    };
    let mut session = LocalChangeSession::start(
        req.circle_id.to_string(),
        req.base_snapshot.to_string(),
        mode,
    );
    session.requested_agent = Some(req.agent_name.to_string());
    session.actor_id = Some(req.agent_name.to_string());
    tracing::info!(
        "[agent] launching `{}` ({:?}) session={} resume={:?} task_len={}",
        req.agent_name,
        req.cmd.driver,
        session.session_id,
        req.resume,
        req.task.len()
    );

    let run_dir = working_dir(req.workspace, req.cmd.working_dir.as_deref());

    let (detail, reply, acp_session_id) = match req.cmd.driver {
        Driver::Argv => (run_argv(req.cmd, req.task, &run_dir).await?, None, None),
        Driver::Acp => {
            let r = run_acp(req.cmd, req.initiator, req.task, &run_dir, req.resume).await?;
            (r.detail, r.reply, r.acp_session_id)
        }
    };

    session.finish();
    Ok(LaunchOutcome {
        session_id: session.session_id,
        mode,
        detail,
        reply,
        acp_session_id,
    })
}

struct AcpRun {
    detail: String,
    reply: Option<String>,
    acp_session_id: Option<String>,
}

async fn run_argv(cmd: &AgentCommand, task: &str, run_dir: &Path) -> Result<String> {
    let rendered = cmd.render(task);
    let (program, args) = rendered.split_first().context("empty agent command")?;
    let status = super::spawn::command(program, args)
        .current_dir(run_dir)
        .status()
        .await
        .with_context(|| format!("failed to spawn agent `{program}`"))?;
    Ok(format!("exit {status}"))
}

async fn run_acp(
    cmd: &AgentCommand,
    _initiator: Initiator,
    task: &str,
    run_dir: &Path,
    resume: Option<&str>,
) -> Result<AcpRun> {
    // Always allow the agent to act *within the workspace* — that is its job,
    // and enoxian captures whatever it writes as accepted proposal history.
    // Deny-at-tool-call would just make a mentioned agent unable to do anything.
    let reply = Arc::new(Mutex::new(ReplyBuf::default()));
    // Start with capture OFF so the history replayed during session/load is not
    // mistaken for the current reply. Turned on just before we prompt.
    let capturing = Arc::new(AtomicBool::new(false));
    let hooks = PolicyHooks {
        allow: true,
        reply: reply.clone(),
        capturing: capturing.clone(),
    };

    let mut acp = AcpSession::start(&cmd.command, run_dir, hooks, resume)
        .await
        .context("ACP handshake failed")?;

    // Now capture only the reply to *this* prompt.
    capturing.store(true, Ordering::Relaxed);
    let result = acp.prompt(task).await;
    let acp_session_id = acp.session_id().map(str::to_string);
    acp.shutdown().await;
    let turn = result.context("ACP prompt turn failed")?;

    // The reply is the agent's final message this turn (see ReplyBuf).
    let text = std::mem::take(&mut *reply.lock().unwrap()).into_reply();
    Ok(AcpRun {
        detail: format!("stop_reason={}", turn.stop_reason),
        reply: text,
        acp_session_id,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_is_final_message_not_concatenation() {
        let mut buf = ReplyBuf::default();
        // A greeting message (streamed as chunks), then a boundary, then the
        // real answer. The reply must be the answer, not "greeting...answer".
        buf.push_chunk("Hello! ");
        buf.push_chunk("What can I do for you?");
        buf.boundary(); // e.g. a tool call happened
        buf.push_chunk("Done — I created ");
        buf.push_chunk("test.txt.");
        assert_eq!(
            buf.into_reply().as_deref(),
            Some("Done — I created test.txt.")
        );
    }

    #[test]
    fn single_message_survives() {
        let mut buf = ReplyBuf::default();
        buf.push_chunk("just one message");
        assert_eq!(buf.into_reply().as_deref(), Some("just one message"));
    }

    #[test]
    fn empty_trailing_messages_ignored() {
        let mut buf = ReplyBuf::default();
        buf.push_chunk("the answer");
        buf.boundary();
        buf.push_chunk("   "); // whitespace-only trailing message
        assert_eq!(buf.into_reply().as_deref(), Some("the answer"));
    }

    #[test]
    fn no_output_is_none() {
        assert_eq!(ReplyBuf::default().into_reply(), None);
    }
}
