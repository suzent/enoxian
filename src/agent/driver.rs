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
//! Every run has a durable change record and per-agent conversation lease.
//! Supported ACP writes carry operation evidence; unmediated writes remain
//! unattributed history. Conversation IDs are persisted before releasing the lease.

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
    /// An unaddressed turn this device volunteered for (engagement spec §2).
    /// Nobody asked, so whatever it writes is held for review rather than
    /// accepted outright.
    Ambient,
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
    workspace: PathBuf,
    circle_dir: PathBuf,
    session: LocalChangeSession,
    coordination: Option<crate::state::AppState>,
    allow: bool,
    /// Segmented capture of the agent's streamed messages. `&self`-only trait,
    /// hence the shared mutable cell.
    reply: Arc<Mutex<ReplyBuf>>,
    /// When false, streamed text is ignored (during `session/load` history
    /// replay, before the current prompt turn).
    capturing: Arc<AtomicBool>,
}

impl ClientHooks for PolicyHooks {
    fn on_spawn(&self, pid: u32) -> Result<()> {
        crate::proposal::runs::atomic_json(
            &self
                .circle_dir
                .join("managed_runs")
                .join(format!("{}.json", self.session.session_id)),
            &crate::proposal::runs::RunRecord {
                writes_consumed: false,
                session: self.session.clone(),
                owner_pid: std::process::id(),
                child_pid: Some(pid),
                interrupted: false,
            },
        )
    }
    fn run_id(&self) -> Option<&str> {
        Some(&self.session.session_id)
    }
    fn write_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        use yrs::{ReadTxn, Transact};
        let _order = crate::proposal::evidence::WRITE_ORDER
            .lock()
            .map_err(|_| anyhow::anyhow!("write journal poisoned"))?;
        let mut owns_lock = false;
        let annotate = |detail| {
            if let Some(state) = &self.coordination {
                if let Some(inbox) = state
                    .execution_inbox
                    .read()
                    .unwrap()
                    .as_ref()
                    .and_then(std::sync::Weak::upgrade)
                {
                    let _ = inbox.annotate_running(&self.session.session_id, detail);
                }
            }
        };
        let txn = self
            .coordination
            .as_ref()
            .map(|state| state.control.try_transact())
            .transpose()
            .map_err(|_| anyhow::anyhow!("Circle busy; retry write"))?;
        if let (Some(state), Some(txn)) = (&self.coordination, &txn) {
            let rel = path
                .strip_prefix(&self.workspace)?
                .to_string_lossy()
                .replace('\\', "/");
            if let Some(log) = txn.get_array(crate::control::LOCK_LOG_KEY) {
                if crate::control::arbitration::is_locked_by_other_run(
                    &log,
                    txn,
                    &rel,
                    self.session.actor_id.as_deref().unwrap_or(""),
                    &state.peer_id,
                    Some(&self.session.session_id),
                ) {
                    annotate(Some(format!("waiting for file lock: {rel}")));
                    anyhow::bail!("file locked by another agent or run: {rel}");
                }
                owns_lock =
                    crate::control::arbitration::compute_lock_holders(&log, txn).contains_key(&rel);
            }
        }
        // chmod is a cooperative signal, not per-process isolation. The
        // owning native hook may write, then restores the visible lock mode.
        let permissions = std::fs::metadata(path)
            .ok()
            .map(|m| m.permissions())
            .filter(|p| p.readonly());
        if let Some(original) = &permissions {
            anyhow::ensure!(owns_lock, "read-only file without a verified owned lock");
            let mut writable = original.clone();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                writable.set_mode(writable.mode() | 0o200);
            }
            #[cfg(not(unix))]
            writable.set_readonly(false);
            std::fs::set_permissions(path, writable)?;
        }
        annotate(None);
        let result = crate::proposal::evidence::write_ordered(
            &self.workspace,
            &self.circle_dir,
            &self.session,
            path,
            content,
        );
        if let Some(original) = permissions {
            std::fs::set_permissions(path, original)?;
        }
        result
    }
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
    pub run_id: Option<&'a str>,
    pub trigger_id: Option<&'a str>,
    pub coordination: Option<crate::state::AppState>,
    pub agent_name: &'a str,
    pub cmd: &'a AgentCommand,
    /// The full prompt handed to the agent (task + any injected world context).
    pub task: &'a str,
    pub workspace: &'a Path,
    pub base_snapshot: &'a str,
    pub circle_id: &'a str,
    pub circle_dir: &'a Path,
    /// Token issued by this Circle's daemon for coordination CLI calls made
    /// from the managed process tree. It is never placed in the prompt.
    pub actor_token: Option<&'a str>,
    pub initiator: Initiator,
    /// Agents that relayed this work, innermost last; empty when a person
    /// asked directly. Recorded on the resulting proposal.
    pub relay_path: Vec<String>,
    /// Prior ACP session id to resume, if one is remembered for this agent.
    pub resume: Option<&'a str>,
}

#[derive(Clone, Copy)]
struct ManagedActor<'a> {
    agent_id: &'a str,
    circle_id: &'a str,
    token: Option<&'a str>,
}

/// Launch a permitted agent, running the given task under a change session.
/// Returns once the agent finishes its work.
pub async fn launch(req: LaunchRequest<'_>) -> Result<LaunchOutcome> {
    let mode = match req.initiator {
        // A managed run enoxian owns the process tree for → verified process.
        Initiator::Local | Initiator::RemoteMember => SessionMode::ManagedProcess,
        // Still a managed process, but one nobody asked for. The distinct mode
        // is what lets the proposal engine hold its writes for review.
        Initiator::Ambient => SessionMode::AmbientTriggered,
    };
    let mut session = LocalChangeSession::start(
        req.circle_id.to_string(),
        req.base_snapshot.to_string(),
        mode,
    );
    if let Some(id) = req.run_id {
        crate::proposal::validate_storage_id("run", id)?;
        session.session_id = id.into();
    }
    session.trigger_id = req.trigger_id.map(str::to_string);
    session.requested_agent = Some(req.agent_name.to_string());
    session.relay_path = req.relay_path.clone();
    session.actor_id = Some(req.agent_name.to_string());
    let mut lease = crate::proposal::runs::RunLease::acquire(req.circle_dir, session.clone())?;
    if let (Some(state), Some(token)) = (&req.coordination, req.actor_token) {
        state.actor_tokens.bind_run(token, &session.session_id);
    }
    let _device = crate::proposal::runs::DeviceLease::acquire(
        &crate::proposal::runs::device_slots_dir()?,
        &req.circle_dir
            .join("managed_runs")
            .join(format!("{}.json", session.session_id)),
        super::config::AgentConfig::load().max_concurrent_runs,
    )
    .await?;
    tracing::info!(
        "[agent] launching `{}` ({:?}) session={} resume={:?} task_len={}",
        req.agent_name,
        req.cmd.driver,
        session.session_id,
        req.resume,
        req.task.len()
    );

    let run_dir = working_dir(req.workspace, req.cmd.working_dir.as_deref());
    let actor = ManagedActor {
        agent_id: req.agent_name,
        circle_id: req.circle_id,
        token: req.actor_token,
    };

    let memory = super::memory::load(req.circle_dir, req.agent_name);
    let resume = memory
        .as_ref()
        .map(|r| r.session_id.as_str())
        .or(req.resume);
    let run_result = match req.cmd.driver {
        Driver::Argv => run_argv(req.cmd, req.task, &run_dir, actor, &mut lease)
            .await
            .map(|detail| (detail, None, None)),
        Driver::Acp => run_acp(
            req.cmd,
            req.initiator,
            req.task,
            &run_dir,
            resume,
            actor,
            &mut lease,
            req.workspace,
            req.circle_dir,
            req.coordination.clone(),
        )
        .await
        .map(|r| (r.detail, r.reply, r.acp_session_id)),
    };

    if let Ok((_, _, Some(sid))) = &run_result {
        super::memory::save_session(req.circle_dir, req.agent_name, sid)?;
    }
    lease.finish()?;
    if let Some(state) = &req.coordination {
        if let Err(error) = crate::proposal::runs::release_finished_locks(state) {
            tracing::warn!("lock cleanup deferred: {error}");
        }
    }
    let (detail, reply, acp_session_id) = run_result?;
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

struct ProcessTree(u32);
impl Drop for ProcessTree {
    fn drop(&mut self) {
        super::spawn::kill_tree(self.0);
    }
}

async fn run_argv(
    cmd: &AgentCommand,
    task: &str,
    run_dir: &Path,
    actor: ManagedActor<'_>,
    lease: &mut crate::proposal::runs::RunLease,
) -> Result<String> {
    let rendered = cmd.render(task);
    let (program, args) = rendered.split_first().context("empty agent command")?;
    let mut command = super::spawn::command(program, args);
    super::spawn::apply_actor_env(&mut command, actor.agent_id, actor.circle_id, actor.token);
    command.env("ENOXIAN_RUN_ID", &lease.record.session.session_id);
    let mut child = command
        .current_dir(run_dir)
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("failed to spawn agent `{program}`"))?;
    let _tree = ProcessTree(child.id().context("child PID missing")?);
    lease.child(_tree.0)?;
    let status = child.wait().await?;
    anyhow::ensure!(status.success(), "agent `{program}` exited with {status}");
    Ok(format!("exit {status}"))
}

#[allow(clippy::too_many_arguments)] // Explicit per-run context stays scoped to this adapter.
async fn run_acp(
    cmd: &AgentCommand,
    _initiator: Initiator,
    task: &str,
    run_dir: &Path,
    resume: Option<&str>,
    actor: ManagedActor<'_>,
    lease: &mut crate::proposal::runs::RunLease,
    workspace: &Path,
    circle_dir: &Path,
    coordination: Option<crate::state::AppState>,
) -> Result<AcpRun> {
    // Always allow the agent to act *within the workspace* — that is its job,
    // and enoxian captures whatever it writes as accepted proposal history.
    // Deny-at-tool-call would just make a mentioned agent unable to do anything.
    let reply = Arc::new(Mutex::new(ReplyBuf::default()));
    // Start with capture OFF so the history replayed during session/load is not
    // mistaken for the current reply. Turned on just before we prompt.
    let capturing = Arc::new(AtomicBool::new(false));
    let hooks = PolicyHooks {
        coordination: coordination.clone(),
        workspace: workspace.canonicalize()?,
        circle_dir: circle_dir.to_path_buf(),
        session: lease.record.session.clone(),
        allow: true,
        reply: reply.clone(),
        capturing: capturing.clone(),
    };

    let mut acp = AcpSession::start(
        &cmd.command,
        run_dir,
        hooks,
        resume,
        actor.agent_id,
        actor.circle_id,
        actor.token,
    )
    .await
    .context("ACP handshake failed")?;

    if let Some(pid) = acp.process_id() {
        lease.child(pid)?;
    }

    // Now capture only the reply to *this* prompt.
    capturing.store(true, Ordering::Relaxed);
    let prompt = if resume.is_some() && !acp.was_resumed() {
        let context = coordination.as_ref().map(|state| super::context::recovery_context(state, actor.agent_id))
            .unwrap_or_else(|| "Previous ACP session could not be restored; private conversation memory is unavailable. Retrieve Circle chat if needed.\n\n".into());
        format!("{context}{task}")
    } else {
        task.to_string()
    };
    let result = acp.prompt(&prompt).await;
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
