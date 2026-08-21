//! Minimal Agent Client Protocol (ACP) client.
//!
//! enoxian is the **client**; the spawned coding agent is the **agent**. We
//! speak newline-delimited JSON-RPC 2.0 over the child's stdin/stdout:
//!
//! ```text
//! client -> agent:  initialize        (advertise fs capabilities)
//! client -> agent:  session/new       (cwd = workspace)
//! client -> agent:  session/prompt    (the mention's task text)
//! agent  -> client: fs/read_text_file, fs/write_text_file,
//!                   session/request_permission, session/update
//! ```
//!
//! We keep the surface small on purpose: text prompts, a permission callback
//! that defers to the acceptance policy, and client-mediated file I/O confined
//! to the workspace. Writes land on the real workspace so the ambient proposal
//! engine captures them exactly as it captures any other file mutation — the
//! ACP driver adds attribution and a real turn lifecycle, not a separate
//! proposal path.
//!
//! See <https://agentclientprotocol.com/protocol/schema>.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::mpsc;

/// The ACP protocol version we implement. Agents may echo it as a string or an
/// integer; we accept either on the response and never hard-fail on a mismatch
/// (forward-compat: a newer agent still speaks v1 methods).
const PROTOCOL_VERSION: u32 = 1;

/// Decision returned by the permission callback the driver installs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    Allow,
    Deny,
}

/// How the client answers the agent's requests. The driver supplies this so
/// the acceptance policy — not the ACP layer — owns the allow/deny decision.
pub trait ClientHooks: Send + 'static {
    /// Called for `session/request_permission`. Return whether to allow.
    fn on_permission(&self, tool: &Value) -> PermissionDecision;
    /// Called for each `session/update` notification (streamed agent output).
    /// Default: log at debug. Override to surface progress.
    fn on_update(&self, _update: &Value) {}
}

/// The discriminator of a `session/update` (`sessionUpdate` or legacy `type`).
pub fn update_kind(update: &Value) -> Option<&str> {
    update
        .get("sessionUpdate")
        .or_else(|| update.get("type"))
        .and_then(Value::as_str)
}

/// Extract the plain text from a `session/update` notification's `update`
/// object, if it carries an agent message chunk. Handles both the
/// `sessionUpdate` discriminator and a plain `type`, and returns `None` for
/// non-text updates (tool calls, plans, etc.).
pub fn agent_message_text(update: &Value) -> Option<String> {
    let kind = update_kind(update);
    // Only accumulate the agent's own message chunks — not tool output or user
    // echoes.
    if !matches!(kind, Some("agent_message_chunk") | Some("agent_message")) {
        return None;
    }
    let content = update.get("content")?;
    // content may be a single {type:text,text} block or an array of blocks.
    match content {
        Value::Array(blocks) => {
            let joined: String = blocks
                .iter()
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect();
            if joined.is_empty() {
                None
            } else {
                Some(joined)
            }
        }
        obj => obj.get("text").and_then(Value::as_str).map(str::to_string),
    }
}

/// Outcome of a completed prompt turn.
#[derive(Debug, Clone)]
pub struct TurnResult {
    /// The agent's stop reason (e.g. "end_turn", "completed", "cancelled").
    pub stop_reason: String,
}

/// A live ACP session against a spawned agent subprocess.
pub struct AcpSession<H: ClientHooks> {
    child: Child,
    stdin: ChildStdin,
    /// Responses to requests we sent, delivered by the reader task.
    resp_rx: mpsc::UnboundedReceiver<Value>,
    /// Agent-initiated requests/notifications, delivered by the reader task.
    req_rx: mpsc::UnboundedReceiver<Value>,
    session_id: Option<String>,
    /// Whether the agent advertised the `loadSession` capability at init.
    load_session_cap: bool,
    workspace: PathBuf,
    hooks: H,
    next_id: u64,
}

impl<H: ClientHooks> AcpSession<H> {
    /// Spawn the agent command and complete the ACP handshake, leaving a
    /// session ready for `prompt`.
    ///
    /// `command` is argv (e.g. a pinned managed `claude-agent-acp` executable).
    /// `workspace` is the absolute directory the agent operates in; all
    /// client-mediated file access is confined to it.
    ///
    /// `resume` is a prior `sessionId` to continue. When present and the agent
    /// advertises the `loadSession` capability, we call `session/load` to
    /// restore the conversation. If loading fails (e.g. the agent no longer
    /// knows that session after a restart), we fall back to `session/new` — so
    /// resume is best-effort continuity, never a hard dependency.
    pub async fn start(
        command: &[String],
        workspace: &Path,
        hooks: H,
        resume: Option<&str>,
    ) -> Result<Self> {
        let (program, args) = command
            .split_first()
            .ok_or_else(|| anyhow!("empty agent command"))?;

        let mut command = super::spawn::command(program, args);
        command
            .current_dir(workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .with_context(|| format!("failed to spawn ACP agent `{program}`"))?;

        let stdin = child.stdin.take().context("child stdin missing")?;
        let stdout = child.stdout.take().context("child stdout missing")?;
        let stderr = child.stderr.take().context("child stderr missing")?;

        // Drain stderr to the log so a crashing agent is diagnosable.
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!("[acp:agent-stderr] {line}");
            }
        });

        // Reader task: split incoming JSON-RPC into responses (have `id` + no
        // `method`) vs inbound requests/notifications (have `method`). Responses
        // go to resp_rx; requests are handled inline because they may need a
        // reply we must write back to stdin — so we forward *raw* request lines
        // on a second channel and service them in `request loop` below.
        let (resp_tx, resp_rx) = mpsc::unbounded_channel::<Value>();
        let (req_tx, req_rx) = mpsc::unbounded_channel::<Value>();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                match serde_json::from_str::<Value>(line) {
                    Ok(msg) => {
                        if msg.get("method").is_some() {
                            let _ = req_tx.send(msg);
                        } else {
                            let _ = resp_tx.send(msg);
                        }
                    }
                    Err(e) => tracing::warn!("[acp] unparseable line from agent: {e}: {line}"),
                }
            }
        });

        let mut session = Self {
            child,
            stdin,
            resp_rx,
            req_rx,
            session_id: None,
            load_session_cap: false,
            workspace: workspace.to_path_buf(),
            hooks,
            next_id: 1,
        };

        session.initialize().await?;

        // Try to resume a prior conversation; fall back to a fresh session.
        match resume {
            Some(prior) if session.load_session_cap => match session.load_session(prior).await {
                Ok(()) => {
                    tracing::info!("[acp] resumed session {prior}");
                }
                Err(e) => {
                    tracing::warn!("[acp] resume of {prior} failed ({e}); starting fresh");
                    session.new_session().await?;
                }
            },
            Some(prior) => {
                tracing::info!("[acp] agent lacks loadSession; ignoring prior session {prior}");
                session.new_session().await?;
            }
            None => session.new_session().await?,
        }
        Ok(session)
    }

    async fn initialize(&mut self) -> Result<()> {
        let result = self
            .call(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "clientCapabilities": {
                        "fs": { "readTextFile": true, "writeTextFile": true },
                        "terminal": false
                    }
                }),
            )
            .await?;
        self.load_session_cap = result
            .get("agentCapabilities")
            .and_then(|c| c.get("loadSession"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        tracing::info!(
            "[acp] initialized (loadSession={}): {}",
            self.load_session_cap,
            compact(&result)
        );
        Ok(())
    }

    /// Resume a prior conversation via `session/load`. The agent replays its
    /// history as `session/update` notifications, which our hooks observe.
    async fn load_session(&mut self, session_id: &str) -> Result<()> {
        let cwd = self
            .workspace
            .to_str()
            .ok_or_else(|| anyhow!("workspace path is not valid UTF-8"))?
            .to_string();
        self.call(
            "session/load",
            json!({ "sessionId": session_id, "cwd": cwd, "mcpServers": [] }),
        )
        .await?;
        self.session_id = Some(session_id.to_string());
        Ok(())
    }

    /// The active session id (for persistence so the next run can resume).
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    async fn new_session(&mut self) -> Result<()> {
        let cwd = self
            .workspace
            .to_str()
            .ok_or_else(|| anyhow!("workspace path is not valid UTF-8"))?
            .to_string();
        let result = self
            .call("session/new", json!({ "cwd": cwd, "mcpServers": [] }))
            .await?;
        let sid = result
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("session/new returned no sessionId"))?
            .to_string();
        tracing::info!("[acp] session {sid} created (cwd={cwd})");
        self.session_id = Some(sid);
        Ok(())
    }

    /// Run one prompt turn to completion. Blocks (awaiting the agent) until it
    /// returns a stop reason — during which the agent may issue fs/permission
    /// requests that we service against the workspace and the policy hooks.
    pub async fn prompt(&mut self, task: &str) -> Result<TurnResult> {
        let sid = self
            .session_id
            .clone()
            .ok_or_else(|| anyhow!("prompt before session/new"))?;
        let result = self
            .call(
                "session/prompt",
                json!({
                    "sessionId": sid,
                    "prompt": [{ "type": "text", "text": task }]
                }),
            )
            .await?;
        let stop_reason = result
            .get("stopReason")
            .and_then(Value::as_str)
            .unwrap_or("completed")
            .to_string();
        Ok(TurnResult { stop_reason })
    }

    /// Terminate the agent subprocess.
    pub async fn shutdown(mut self) {
        let _ = self.stdin.shutdown().await;
        // Kill the whole process tree: `npx` spawns `node`, which spawns the
        // real agent. start_kill() alone would reap only the launcher and orphan
        // the descendants (the stray node/claude processes seen in testing).
        if let Some(pid) = self.child.id() {
            super::spawn::kill_tree(pid);
        }
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
    }

    // ── JSON-RPC plumbing ─────────────────────────────────────────────────────

    /// Send a request and await its response, servicing any agent-initiated
    /// requests that arrive in the meantime.
    async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
        .await?;

        let limit = if method == "session/prompt" {
            Duration::from_secs(30 * 60)
        } else {
            Duration::from_secs(45)
        };
        let response = tokio::time::timeout(limit, async {
          loop {
            tokio::select! {
                // A message from the agent that is our response or an out-of-band one.
                resp = self.resp_rx.recv() => {
                    let resp = resp.ok_or_else(|| anyhow!("agent closed the connection during `{method}`"))?;
                    if resp.get("id").and_then(Value::as_u64) == Some(id) {
                        if let Some(err) = resp.get("error") {
                            return Err(anyhow!("agent error on `{method}`: {}", compact(err)));
                        }
                        return Ok(resp.get("result").cloned().unwrap_or(Value::Null));
                    }
                    // A response to a different id (shouldn't happen with our
                    // serial flow) — ignore.
                }
                // An agent-initiated request/notification we must service.
                req = self.req_rx.recv() => {
                    if let Some(req) = req {
                        self.handle_agent_request(req).await?;
                    }
                }
            }
          }
        }).await;
        response
            .map_err(|_| anyhow!("ACP `{method}` timed out after {} seconds", limit.as_secs()))?
    }

    /// Handle a request or notification the agent sent us.
    async fn handle_agent_request(&mut self, msg: Value) -> Result<()> {
        let method = msg
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let id = msg.get("id").cloned();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);

        match method {
            "session/update" => {
                self.hooks
                    .on_update(&params.get("update").cloned().unwrap_or(Value::Null));
                // Notification — no reply.
            }
            "fs/read_text_file" => {
                let result = self.fs_read(&params);
                self.reply(id, result).await?;
            }
            "fs/write_text_file" => {
                let result = self.fs_write(&params);
                self.reply(id, result).await?;
            }
            "session/request_permission" => {
                if let Some(opts) = params.get("options") {
                    tracing::debug!("[acp] request_permission options: {}", compact(opts));
                }
                let tool = params.get("toolCall").cloned().unwrap_or(Value::Null);
                let decision = self.hooks.on_permission(&tool);
                let outcome = match decision {
                    PermissionDecision::Allow => {
                        pick_option(&params, &["allow_once", "allow_always", "allow"])
                    }
                    PermissionDecision::Deny => {
                        pick_option(&params, &["reject_once", "reject_always", "reject"])
                    }
                };
                let result_value = match outcome {
                    Some(option_id) => {
                        json!({ "outcome": { "outcome": "selected", "optionId": option_id } })
                    }
                    None => json!({ "outcome": { "outcome": "cancelled" } }),
                };
                tracing::debug!("[acp] request_permission reply: {}", compact(&result_value));
                self.reply(id, Ok(result_value)).await?;
            }
            other => {
                // Unknown request: reply with a method-not-found error if it
                // expects a response; ignore pure notifications.
                if id.is_some() {
                    tracing::debug!("[acp] unhandled agent method `{other}` — replying not-found");
                    self.reply(id, Err(anyhow!("method not found"))).await?;
                }
            }
        }
        Ok(())
    }

    /// Read a workspace file for the agent. Confined to the workspace.
    fn fs_read(&self, params: &Value) -> Result<Value> {
        let abs = self.resolve_in_workspace(params.get("path").and_then(Value::as_str))?;
        let content = std::fs::read_to_string(&abs)
            .with_context(|| format!("fs/read_text_file: {}", abs.display()))?;

        // Optional line/limit windowing.
        let line = params.get("line").and_then(Value::as_u64);
        let limit = params.get("limit").and_then(Value::as_u64);
        let windowed = match (line, limit) {
            (Some(start), lim) => {
                let start = start.saturating_sub(1) as usize;
                let it = content.lines().skip(start);
                let selected: Vec<&str> = match lim {
                    Some(n) => it.take(n as usize).collect(),
                    None => it.collect(),
                };
                selected.join("\n")
            }
            _ => content,
        };
        Ok(json!({ "content": windowed }))
    }

    /// Write a workspace file for the agent. Confined to the workspace. The
    /// ambient proposal engine picks the write up from disk as agent evidence.
    fn fs_write(&self, params: &Value) -> Result<Value> {
        let abs = self.resolve_in_workspace(params.get("path").and_then(Value::as_str))?;
        let content = params
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("fs/write_text_file: missing content"))?;
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(&abs, content)
            .with_context(|| format!("fs/write_text_file: {}", abs.display()))?;
        tracing::info!("[acp] agent wrote {}", abs.display());
        Ok(json!({}))
    }

    /// Resolve an agent-supplied path and reject anything escaping the
    /// workspace (path traversal / absolute paths outside the tree).
    fn resolve_in_workspace(&self, path: Option<&str>) -> Result<PathBuf> {
        let raw = path.ok_or_else(|| anyhow!("missing path"))?;
        let candidate = PathBuf::from(raw);
        let abs = if candidate.is_absolute() {
            candidate
        } else {
            self.workspace.join(candidate)
        };
        // Normalize without touching the filesystem, then confirm containment.
        let normalized = normalize(&abs);
        let root = normalize(&self.workspace);
        if !normalized.starts_with(&root) {
            bail!("path {raw} escapes the workspace");
        }
        Ok(normalized)
    }

    async fn reply(&mut self, id: Option<Value>, result: Result<Value>) -> Result<()> {
        let Some(id) = id else { return Ok(()) };
        let msg = match result {
            Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
            Err(e) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32000, "message": e.to_string() }
            }),
        };
        self.send(msg).await
    }

    async fn send(&mut self, msg: Value) -> Result<()> {
        let mut line = serde_json::to_string(&msg)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }
}

impl<H: ClientHooks> Drop for AcpSession<H> {
    fn drop(&mut self) {
        if let Some(pid) = self.child.id() {
            super::spawn::kill_tree(pid);
        }
    }
}

/// Choose the first offered permission option whose `kind` matches one of the
/// preferred kinds, falling back to the first option of any kind.
fn pick_option(params: &Value, preferred_kinds: &[&str]) -> Option<String> {
    let options = params.get("options").and_then(Value::as_array)?;
    for want in preferred_kinds {
        for opt in options {
            if opt.get("kind").and_then(Value::as_str) == Some(want) {
                if let Some(id) = opt.get("optionId").and_then(Value::as_str) {
                    return Some(id.to_string());
                }
            }
        }
    }
    options
        .first()
        .and_then(|o| o.get("optionId").and_then(Value::as_str))
        .map(str::to_string)
}

/// Lexical path normalization (resolves `.` / `..` without hitting disk).
fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn compact(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_option_prefers_kind_then_falls_back() {
        let params = json!({
            "options": [
                { "optionId": "a", "name": "Allow once", "kind": "allow_once" },
                { "optionId": "r", "name": "Reject", "kind": "reject_once" }
            ]
        });
        assert_eq!(pick_option(&params, &["allow_once"]).as_deref(), Some("a"));
        assert_eq!(pick_option(&params, &["reject_once"]).as_deref(), Some("r"));
        // Unknown preference falls back to the first option.
        assert_eq!(pick_option(&params, &["nope"]).as_deref(), Some("a"));
    }

    #[test]
    fn normalize_resolves_traversal() {
        assert_eq!(normalize(Path::new("/a/b/../c")), PathBuf::from("/a/c"));
        assert_eq!(normalize(Path::new("/a/./b")), PathBuf::from("/a/b"));
    }

    #[test]
    fn extracts_agent_message_text() {
        // Single content block, `sessionUpdate` discriminator.
        let u = json!({
            "sessionUpdate": "agent_message_chunk",
            "content": { "type": "text", "text": "hello " }
        });
        assert_eq!(agent_message_text(&u).as_deref(), Some("hello "));

        // Array of blocks, `type` discriminator.
        let u = json!({
            "type": "agent_message",
            "content": [ { "type": "text", "text": "a" }, { "type": "text", "text": "b" } ]
        });
        assert_eq!(agent_message_text(&u).as_deref(), Some("ab"));

        // Non-message updates yield nothing.
        let tool = json!({ "sessionUpdate": "tool_call", "content": { "text": "x" } });
        assert_eq!(agent_message_text(&tool), None);
    }
}
