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
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use super::liveness::Activity;

/// Supported major protocol version.
const PROTOCOL_VERSION: u32 = 1;

const MAX_FRAME: usize = 1024 * 1024;
const MAX_BUFFER: usize = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
#[error("ACP request failed (code {code}): {message}")]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

pub fn auth_required(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<RpcError>()
        .is_some_and(|error| error.code == -32000)
}

struct Inbound {
    value: Value,
    _budget: OwnedSemaphorePermit,
}

async fn read_frame<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Option<(Value, usize)>> {
    loop {
        let mut line = Vec::new();
        let n = (&mut *reader)
            .take((MAX_FRAME + 1) as u64)
            .read_until(b'\n', &mut line)
            .await?;
        if n == 0 {
            return Ok(None);
        }
        if n > MAX_FRAME {
            bail!("ACP frame exceeds 1 MiB");
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        // The subprocess fixture runs inside Rust's test harness.
        #[cfg(test)]
        if line.starts_with(b"running 1 test") {
            continue;
        }
        let value = serde_json::from_slice(&line).context("Invalid ACP JSON frame")?;
        return Ok(Some((value, n)));
    }
}

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
    fn on_spawn(&self, _pid: u32) -> Result<()> {
        Ok(())
    }
    fn run_id(&self) -> Option<&str> {
        None
    }
    fn write_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
        Ok(())
    }
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
    /// The tokens the prompt used, if the agent reported them.
    pub tokens: Option<super::usage::Tokens>,
}

/// A live ACP session against a spawned agent subprocess.
pub struct AcpSession<H: ClientHooks> {
    child: Child,
    stdin: ChildStdin,
    /// Responses to requests we sent, delivered by the reader task.
    resp_rx: mpsc::Receiver<Inbound>,
    /// Agent-initiated requests/notifications, delivered by the reader task.
    req_rx: mpsc::Receiver<Inbound>,
    transport_error: Arc<Mutex<Option<String>>>,
    reader: tokio::task::JoinHandle<()>,
    stderr_reader: tokio::task::JoinHandle<()>,
    pub auth_methods: Vec<Value>,
    output_bytes: usize,
    session_id: Option<String>,
    /// Whether the agent advertised the `loadSession` capability at init.
    load_session_cap: bool,
    resumed: bool,
    workspace: PathBuf,
    hooks: H,
    next_id: u64,
    /// What the agent has been doing, for whoever is watching the turn.
    activity: Option<Arc<Mutex<Activity>>>,
    /// How long an agent gets to wind down after `session/cancel`.
    cancel_grace: Duration,
    /// Set once `session/cancel` is sent for the current prompt.
    cancelling: bool,
}

/// Handshake requests are bounded: an agent that cannot even start is broken,
/// not busy. A prompt turn is not — see [`AcpSession::prompt_until`].
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(45);

/// How long an agent has to end a turn after being asked to stop, before its
/// process is killed.
pub const CANCEL_GRACE: Duration = Duration::from_secs(30);

impl<H: ClientHooks> AcpSession<H> {
    /// Record what the agent does from now on into `activity`.
    pub fn set_activity(&mut self, activity: Arc<Mutex<Activity>>) {
        self.activity = Some(activity);
    }

    pub fn was_resumed(&self) -> bool {
        self.resumed
    }
    pub fn process_id(&self) -> Option<u32> {
        self.child.id()
    }

    /// Spawn the agent command and complete the ACP handshake, leaving a
    /// session ready for `prompt`.
    ///
    /// `command` is argv (e.g. a pinned managed `claude-agent-acp` executable).
    /// `workspace` is the absolute directory the agent operates in; all
    /// client-mediated file access is confined to it.
    ///
    /// `resume` is a prior `sessionId` to continue. When present and the agent
    /// advertises the `loadSession` capability, we call `session/load` to
    /// restore the conversation. Errors preserve the saved session rather than
    /// silently replacing it with an empty conversation.
    pub async fn start(
        command: &[String],
        workspace: &Path,
        hooks: H,
        resume: Option<&str>,
        agent_id: &str,
        circle_id: &str,
        actor_token: Option<&str>,
    ) -> Result<Self> {
        let mut session = Self::connect(
            command,
            workspace,
            hooks,
            agent_id,
            circle_id,
            actor_token,
            false,
        )
        .await?;
        match resume {
            Some(prior) if session.load_session_cap => {
                session.load_session(prior).await.context(
                    "Could not restore ACP session; the saved conversation was not replaced",
                )?;
                session.resumed = true;
            }
            _ => session.new_session().await?,
        }
        Ok(session)
    }

    pub async fn connect(
        command: &[String],
        workspace: &Path,
        hooks: H,
        agent_id: &str,
        circle_id: &str,
        actor_token: Option<&str>,
        terminal_auth: bool,
    ) -> Result<Self> {
        let (program, args) = command
            .split_first()
            .ok_or_else(|| anyhow!("empty agent command"))?;

        let mut command = super::spawn::command(program, args);
        super::spawn::apply_actor_env(&mut command, agent_id, circle_id, actor_token);
        if let Some(id) = hooks.run_id() {
            command.env("ENOXIAN_RUN_ID", id);
        }
        command
            .current_dir(workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .with_context(|| format!("failed to spawn ACP agent `{program}`"))?;

        hooks.on_spawn(child.id().context("child PID missing")?)?;
        let stdin = child.stdin.take().context("child stdin missing")?;
        let stdout = child.stdout.take().context("child stdout missing")?;
        let stderr = child.stderr.take().context("child stderr missing")?;

        // Drain without logging credential-bearing provider output.
        let stderr_reader = tokio::spawn(async move {
            let _ = tokio::io::copy(&mut BufReader::new(stderr), &mut tokio::io::sink()).await;
        });
        let (resp_tx, resp_rx) = mpsc::channel::<Inbound>(8);
        let (req_tx, req_rx) = mpsc::channel::<Inbound>(256);
        let transport_error = Arc::new(Mutex::new(None));
        let error = transport_error.clone();
        let reader = tokio::spawn(async move {
            let budget = Arc::new(Semaphore::new(MAX_BUFFER));
            let mut input = BufReader::new(stdout);
            let result: Result<()> = async {
                while let Some((value, bytes)) = read_frame(&mut input).await? {
                    let permit = budget
                        .clone()
                        .try_acquire_many_owned(bytes as u32)
                        .map_err(|_| anyhow!("ACP queued output exceeds 16 MiB"))?;
                    let is_request = value.get("method").is_some();
                    let frame = Inbound {
                        value,
                        _budget: permit,
                    };
                    let channel = if is_request { &req_tx } else { &resp_tx };
                    channel
                        .try_send(frame)
                        .map_err(|_| anyhow!("ACP message queue overflow"))?;
                }
                Ok(())
            }
            .await;
            if let Err(reason) = result {
                *error.lock().unwrap() = Some(reason.to_string());
            }
        });

        let mut session = Self {
            child,
            stdin,
            resp_rx,
            req_rx,
            transport_error,
            reader,
            stderr_reader,
            auth_methods: Vec::new(),
            output_bytes: 0,
            session_id: None,
            load_session_cap: false,
            resumed: false,
            workspace: workspace.to_path_buf(),
            hooks,
            next_id: 1,
            activity: None,
            cancel_grace: CANCEL_GRACE,
            cancelling: false,
        };

        session.initialize(terminal_auth).await?;
        Ok(session)
    }

    async fn initialize(&mut self, terminal_auth: bool) -> Result<()> {
        let result = self
            .call(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "clientInfo": {"name": "enoxian", "version": env!("CARGO_PKG_VERSION")},
                    "clientCapabilities": {
                        "fs": { "readTextFile": true, "writeTextFile": true },
                        "terminal": false,
                        "auth": { "terminal": terminal_auth }
                    }
                }),
            )
            .await?;
        let version = result.get("protocolVersion");
        let major = version.and_then(Value::as_u64).or_else(|| {
            version
                .and_then(Value::as_str)
                .and_then(|value| value.parse::<u64>().ok())
        });
        if major != Some(PROTOCOL_VERSION as u64) {
            bail!("Unsupported ACP protocol version");
        }
        self.auth_methods = result
            .get("authMethods")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        self.load_session_cap = result
            .get("agentCapabilities")
            .and_then(|c| c.get("loadSession"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
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

    pub async fn new_session(&mut self) -> Result<()> {
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

    /// Run one prompt turn until the agent ends it — during which the agent may
    /// issue fs/permission requests that we service against the workspace and
    /// the policy hooks.
    ///
    /// There is no time limit. ACP has no heartbeat, and a turn that says
    /// nothing for minutes is often a long tool call, which is work; timing it
    /// out threw that work away. The turn ends when the agent ends it, when its
    /// process exits, or when `stop` fires: then the agent is sent
    /// `session/cancel`, which obliges it to wind down and answer `cancelled`,
    /// and gets [`CANCEL_GRACE`] to do so before this returns an error and the
    /// caller kills it.
    pub async fn prompt_until(
        &mut self,
        task: &str,
        stop: &CancellationToken,
    ) -> Result<TurnResult> {
        let sid = self
            .session_id
            .clone()
            .ok_or_else(|| anyhow!("prompt before session/new"))?;
        self.cancelling = false;
        self.output_bytes = 0;
        let result = self
            .exchange(
                "session/prompt",
                json!({
                    "sessionId": sid,
                    "prompt": [{ "type": "text", "text": task }]
                }),
                Some(stop),
            )
            .await?;
        let stop_reason = result
            .get("stopReason")
            .and_then(Value::as_str)
            .unwrap_or("completed")
            .to_string();
        Ok(TurnResult {
            stop_reason,
            tokens: super::usage::parse_tokens(&result),
        })
    }

    pub async fn authenticate(&mut self, method_id: &str) -> Result<()> {
        let method = self
            .auth_methods
            .iter()
            .find(|m| m["id"].as_str() == Some(method_id))
            .context("Authentication method was not advertised")?;
        if method
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("agent")
            != "agent"
        {
            bail!("This method requires terminal authentication");
        }
        self.exchange("authenticate", json!({"methodId": method_id}), None)
            .await?;
        Ok(())
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

    /// Send a handshake request and await its response.
    async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        tokio::time::timeout(HANDSHAKE_TIMEOUT, self.exchange(method, params, None))
            .await
            .map_err(|_| {
                anyhow!(
                    "ACP `{method}` timed out after {} seconds",
                    HANDSHAKE_TIMEOUT.as_secs()
                )
            })?
    }

    /// Send a request and await its response, servicing any agent-initiated
    /// requests that arrive in the meantime. When `stop` fires, ask the agent
    /// to cancel and give it [`Self::cancel_grace`] to answer.
    async fn exchange(
        &mut self,
        method: &str,
        params: Value,
        stop: Option<&CancellationToken>,
    ) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
        .await?;

        let never = CancellationToken::new();
        let stop = stop.unwrap_or(&never);
        let mut requests_open = true;
        let mut grace: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;
        loop {
            if let Some(error) = self.transport_error.lock().unwrap().as_ref() {
                bail!("{error}");
            }
            tokio::select! {
                _ = stop.cancelled(), if !self.cancelling => {
                    self.cancelling = true;
                    self.send_cancel().await?;
                    grace = Some(Box::pin(tokio::time::sleep(self.cancel_grace)));
                }
                _ = async { grace.as_mut().unwrap().await }, if grace.is_some() => {
                    bail!(
                        "agent did not stop within {} seconds of session/cancel",
                        self.cancel_grace.as_secs()
                    );
                }
                // A message from the agent that is our response or an out-of-band one.
                resp = self.resp_rx.recv() => {
                    let resp = resp.ok_or_else(|| anyhow!(self.transport_error.lock().unwrap().clone()
                        .unwrap_or_else(|| format!("agent closed the connection during `{method}`"))))?.value;
                    if resp.get("id").and_then(Value::as_u64) == Some(id) {
                        // The reader routes lines in order, so whatever the
                        // agent sent before this response is already queued.
                        // `select!` may have picked the response first; the
                        // last reply chunk is often among those (#251).
                        while let Ok(req) = self.req_rx.try_recv() {
                            self.service(req.value).await?;
                        }
                        if let Some(error) = self.transport_error.lock().unwrap().as_ref() {
                            bail!("{error}");
                        }
                        if let Some(err) = resp.get("error") {
                            return Err(RpcError {
                                code: err["code"].as_i64().unwrap_or(-32603),
                                message: err["message"].as_str().unwrap_or("Agent request failed").to_string(),
                            }.into());
                        }
                        return Ok(resp.get("result").cloned().unwrap_or(Value::Null));
                    }
                    // A response to a different id (shouldn't happen with our
                    // serial flow) — ignore.
                }
                // An agent-initiated request/notification we must service.
                req = self.req_rx.recv(), if requests_open => {
                    match req {
                        Some(req) => self.service(req.value).await?,
                        None => requests_open = false,
                    }
                }
            }
        }
    }

    /// Record what the agent is doing and handle its request or notification.
    async fn service(&mut self, req: Value) -> Result<()> {
        if let Some(activity) = &self.activity {
            activity
                .lock()
                .unwrap()
                .observe(&req, chrono::Utc::now().timestamp());
        }
        self.handle_agent_request(req).await
    }

    /// Tell the agent to stop the current turn (a notification; the turn's
    /// `session/prompt` response carries the outcome).
    async fn send_cancel(&mut self) -> Result<()> {
        let Some(sid) = self.session_id.clone() else {
            return Ok(());
        };
        tracing::info!("[acp] asking the agent to stop session {sid}");
        self.send(json!({
            "jsonrpc": "2.0",
            "method": "session/cancel",
            "params": { "sessionId": sid },
        }))
        .await
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
                self.output_bytes += serde_json::to_vec(&params)?.len();
                if self.output_bytes > MAX_BUFFER {
                    bail!("ACP turn output exceeds 16 MiB");
                }
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
            // Once stopping, whatever the agent asks permission for is the
            // turn it was told to end. The spec has the client answer those
            // `cancelled`.
            "session/request_permission" if self.cancelling => {
                self.reply(id, Ok(json!({ "outcome": { "outcome": "cancelled" } })))
                    .await?;
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
                    self.reply(
                        id,
                        Err(RpcError {
                            code: -32601,
                            message: "Method not found".into(),
                        }
                        .into()),
                    )
                    .await?;
                }
            }
        }
        Ok(())
    }

    /// Read a workspace file for the agent. Confined to the workspace.
    fn fs_read(&self, params: &Value) -> Result<Value> {
        let abs = self.resolve_in_workspace(params.get("path").and_then(Value::as_str))?;
        let file = std::fs::File::open(&abs)?;
        let mut content = String::new();
        std::io::Read::read_to_string(
            &mut std::io::Read::take(file, (MAX_FRAME + 1) as u64),
            &mut content,
        )?;
        if content.len() > MAX_FRAME {
            bail!("File exceeds ACP read limit of 1 MiB");
        }

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
        self.hooks.write_file(&abs, content.as_bytes())?;
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
        crate::proposal::canonical_workspace_path(&self.workspace, &normalized)
    }

    async fn reply(&mut self, id: Option<Value>, result: Result<Value>) -> Result<()> {
        let Some(id) = id else { return Ok(()) };
        let msg = match result {
            Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
            Err(e) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": e.downcast_ref::<RpcError>().map_or(-32603, |error| error.code), "message": e.to_string() }
            }),
        };
        self.send(msg).await
    }

    async fn send(&mut self, msg: Value) -> Result<()> {
        let mut line = to_ascii_json(&msg)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }
}

impl<H: ClientHooks> Drop for AcpSession<H> {
    fn drop(&mut self) {
        self.reader.abort();
        self.stderr_reader.abort();
        if let Some(pid) = self.child.id() {
            super::spawn::kill_tree(pid);
        }
    }
}

/// Choose the first offered permission option whose `kind` matches one of the
/// preferred kinds. No matching option means cancellation.
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
    None
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

/// Serialize `msg` as JSON with every non-ASCII character written as a `\uXXXX`
/// escape, so the line we put on the agent's stdin is pure ASCII.
///
/// JSON is defined over text, but this transport is a byte pipe, and an agent
/// that decodes its stdin chunk by chunk can split a multi-byte UTF-8 sequence
/// across two reads. A Python agent doing that with `errors="surrogateescape"`
/// turns the orphaned tail byte into a lone surrogate, which then fails to
/// re-encode: an em dash (`e2 80 94`) split after its second byte is reported
/// back to us as `'utf-8' codec can't encode character '\udc94'`. Our prompts
/// carry em dashes and whatever Unicode members type in chat, so a single
/// unlucky buffer boundary was enough to fail a whole turn.
///
/// Escaping keeps the payload byte-identical in meaning while making every byte
/// ASCII, so no chunk boundary can land inside a character. Correct agents are
/// unaffected: `\uXXXX` is ordinary JSON that every parser already handles.
fn to_ascii_json(msg: &Value) -> Result<String> {
    let raw = serde_json::to_string(msg)?;
    if raw.is_ascii() {
        return Ok(raw);
    }
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii() {
            out.push(ch);
        } else {
            // `encode_utf16` yields one unit for the BMP and a surrogate pair
            // above it, which is exactly what JSON's `\uXXXX` form expects.
            let mut buf = [0u16; 2];
            for unit in ch.encode_utf16(&mut buf) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    Ok(out)
}

fn compact(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn pick_option_never_reverses_the_decision() {
        let params = json!({
            "options": [
                { "optionId": "a", "name": "Allow once", "kind": "allow_once" },
                { "optionId": "r", "name": "Reject", "kind": "reject_once" }
            ]
        });
        assert_eq!(pick_option(&params, &["allow_once"]).as_deref(), Some("a"));
        assert_eq!(pick_option(&params, &["reject_once"]).as_deref(), Some("r"));
        // An unmatched decision must not select an unrelated option.
        assert_eq!(pick_option(&params, &["nope"]), None);
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

    #[test]
    fn outgoing_json_is_ascii_only() {
        // An em dash split across the agent's stdin reads is what produced the
        // lone-surrogate encode failure; escaping keeps every byte ASCII.
        let msg =
            json!({ "prompt": "revert \u{2014} so make focused changes", "emoji": "\u{1f600}" });
        let line = to_ascii_json(&msg).unwrap();
        assert!(
            line.is_ascii(),
            "line still carries multi-byte text: {line}"
        );
        assert!(line.contains("\\u2014"), "{line}");
        // Above the BMP is escaped as a surrogate pair.
        assert!(line.contains("\\ud83d\\ude00"), "{line}");
        // And it still parses back to exactly what we sent.
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap(), msg);
    }

    #[test]
    fn ascii_payloads_are_passed_through_unchanged() {
        let msg = json!({ "sessionId": "s1", "text": "plain \"quoted\" ascii\n" });
        assert_eq!(
            to_ascii_json(&msg).unwrap(),
            serde_json::to_string(&msg).unwrap()
        );
    }

    // ── A real turn against a fake agent ─────────────────────────────────────
    //
    // The test binary doubles as the agent: `fake_acp_agent` below does nothing
    // in a normal run, and speaks ACP on stdin/stdout when started as a child
    // whose agent id is `fake-acp:<mode>` (set only in the child's environment,
    // so it cannot fire in the parent). libtest's own lines are not JSON, and
    // the session skips them like any other unparseable line.

    struct AllowAll;
    impl ClientHooks for AllowAll {
        fn on_permission(&self, _tool: &Value) -> PermissionDecision {
            PermissionDecision::Allow
        }
    }

    /// The command that starts the fake agent; see [`fake_acp_agent`].
    pub(crate) fn fake_agent_command() -> Vec<String> {
        vec![
            std::env::current_exe().unwrap().display().to_string(),
            "agent::acp::tests::fake_acp_agent".into(),
            "--exact".into(),
            "--nocapture".into(),
            "--test-threads=1".into(),
            "-q".into(),
        ]
    }

    /// Generous: it bounds a hang in the test, and asserts nothing about speed.
    const LIVENESS: Duration = Duration::from_secs(60);

    #[test]
    fn fake_acp_agent() {
        use std::io::{BufRead, Write};
        let Some(mode) = std::env::var("ENOXIAN_AGENT_ID")
            .ok()
            .and_then(|id| id.strip_prefix("fake-acp:").map(str::to_string))
        else {
            return;
        };
        let send = |msg: Value| {
            let mut out = std::io::stdout();
            writeln!(out, "{msg}").unwrap();
            out.flush().unwrap();
        };
        let mut prompt = None;
        let mut authenticated = false;
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let id = msg.get("id").cloned();
            match msg["method"].as_str() {
                // An agent that never finishes starting.
                _ if mode == "mute" => {}
                Some("initialize") if mode == "string-version" => {
                    send(json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":"1"}}))
                }
                Some("initialize") if mode == "wrong-version" => {
                    send(json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":2}}))
                }
                Some("initialize") if mode == "resume-auth" => send(
                    json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}}),
                ),
                Some("session/load") if mode == "resume-auth" => send(
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":"Sign in required"}}),
                ),
                Some("initialize") if mode == "auth" => send(
                    json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":1,"authMethods":[{"id":"login","name":"Browser"},{"id":"terminal","type":"terminal"}]}}),
                ),
                Some("authenticate") if mode == "auth" => {
                    authenticated = true;
                    send(json!({"jsonrpc":"2.0","id":id,"result":{}}));
                }
                Some("session/new") if mode == "auth" && !authenticated => send(
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":"Sign in required"}}),
                ),
                Some("initialize") => send(json!({"jsonrpc": "2.0", "id": id,
                    "result": {"protocolVersion": 1, "agentCapabilities": {"loadSession": false}}})),
                Some("session/new") => send(json!({"jsonrpc": "2.0", "id": id,
                    "result": {"sessionId": "fake-session"}})),
                // The whole reply and the end of the turn in one write, as an
                // agent that computes its answer before streaming it does.
                Some("session/prompt") if mode == "burst" => {
                    let chunk = json!({"jsonrpc": "2.0", "method": "session/update", "params": {
                        "sessionId": "fake-session", "update": {
                        "sessionUpdate": "agent_message_chunk",
                        "content": {"type": "text", "text": "the answer"}}}});
                    let done = json!({"jsonrpc": "2.0", "id": id,
                        "result": {"stopReason": "end_turn"}});
                    let mut out = std::io::stdout().lock();
                    out.write_all(format!("{chunk}\n{done}\n").as_bytes())
                        .unwrap();
                    out.flush().unwrap();
                }
                // A usage report between two chunks of the answer, as an
                // agent that reports usage as it goes does.
                Some("session/prompt") if mode == "metered" => {
                    let update = |update: Value| {
                        json!({"jsonrpc": "2.0", "method": "session/update",
                        "params": {"sessionId": "fake-session", "update": update}})
                    };
                    let chunk = |text: &str| {
                        update(json!({"sessionUpdate": "agent_message_chunk",
                        "content": {"type": "text", "text": text}}))
                    };
                    send(chunk("the ans"));
                    send(update(
                        json!({"sessionUpdate": "usage_update", "used": 41_000,
                        "size": 200_000, "cost": {"amount": 0.03, "currency": "USD"}}),
                    ));
                    send(chunk("wer"));
                    send(
                        json!({"jsonrpc": "2.0", "id": id, "result": {"stopReason": "end_turn",
                        "usage": {"inputTokens": 2, "outputTokens": 4, "totalTokens": 31_524,
                        "cachedReadTokens": 0, "cachedWriteTokens": 31_518}}}),
                    );
                }
                // Answer with a conclusion line: `say-<x>` concludes `<x>`;
                // anything after a `#` only tells two such agents apart.
                Some("session/prompt") if mode.starts_with("say-") => {
                    let said = mode["say-".len()..].split('#').next().unwrap_or_default();
                    send(
                        json!({"jsonrpc": "2.0", "method": "session/update", "params": {
                        "sessionId": "fake-session", "update": {
                        "sessionUpdate": "agent_message_chunk",
                        "content": {"type": "text", "text": format!("Worked it out.\nANSWER: {said}")}}}}),
                    );
                    send(json!({"jsonrpc": "2.0", "id": id,
                        "result": {"stopReason": "end_turn"}}));
                }
                // Spend something, then fail the turn.
                Some("session/prompt") if mode == "metered-fail" => {
                    send(
                        json!({"jsonrpc": "2.0", "method": "session/update", "params": {
                        "sessionId": "fake-session", "update": {"sessionUpdate": "usage_update",
                        "used": 9_000, "size": 200_000, "cost": {"amount": 0.8, "currency": "USD"}}}}),
                    );
                    send(json!({"jsonrpc": "2.0", "id": id,
                        "error": {"code": -32603, "message": "model overloaded"}}));
                }
                // Start a long tool call and say nothing more, as an agent in
                // the middle of a build does.
                Some("session/prompt") => {
                    prompt = id;
                    send(
                        json!({"jsonrpc": "2.0", "method": "session/update", "params": {
                        "sessionId": "fake-session", "update": {"sessionUpdate": "tool_call",
                        "toolCallId": "t1", "title": "long build", "kind": "execute",
                        "status": "in_progress"}}}),
                    );
                }
                Some("session/cancel") if mode == "cooperative" => {
                    send(json!({"jsonrpc": "2.0", "id": prompt.take(),
                        "result": {"stopReason": "cancelled"}}));
                }
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn accepts_legacy_string_encoded_major_version() {
        let (session, _dir) = fake_session("string-version").await;
        session.shutdown().await;
    }

    #[tokio::test]
    async fn rejects_unsupported_major_version() {
        let dir = tempfile::tempdir().unwrap();
        let result = AcpSession::start(
            &fake_agent_command(),
            dir.path(),
            AllowAll,
            None,
            "fake-acp:wrong-version",
            "circle",
            None,
        )
        .await;
        assert!(result
            .err()
            .unwrap()
            .to_string()
            .contains("Unsupported ACP protocol version"));
    }

    #[tokio::test]
    async fn authentication_failure_does_not_replace_saved_session() {
        let dir = tempfile::tempdir().unwrap();
        let result = AcpSession::start(
            &fake_agent_command(),
            dir.path(),
            AllowAll,
            Some("saved"),
            "fake-acp:resume-auth",
            "circle",
            None,
        )
        .await;
        assert!(auth_required(
            &result.err().expect("must not silently start fresh")
        ));
    }

    #[tokio::test]
    async fn advertised_agent_login_is_verified_without_a_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = AcpSession::connect(
            &fake_agent_command(),
            dir.path(),
            AllowAll,
            "fake-acp:auth",
            "circle",
            None,
            false,
        )
        .await
        .unwrap();
        assert!(auth_required(&session.new_session().await.unwrap_err()));
        assert!(session.authenticate("terminal").await.is_err());
        assert!(session.authenticate("invented").await.is_err());
        session.authenticate("login").await.unwrap();
        session.new_session().await.unwrap();
        session.shutdown().await;
    }

    #[tokio::test]
    async fn framing_rejects_oversized_or_invalid_output_without_echoing_it() {
        let oversized = vec![b'x'; MAX_FRAME + 100];
        let error = read_frame(&mut oversized.as_slice()).await.err().unwrap();
        assert!(error.to_string().contains("exceeds"));
        let error = read_frame(&mut b"secret-invalid-json\n".as_slice())
            .await
            .err()
            .unwrap();
        assert!(!error.to_string().contains("secret"));
        let frame = read_frame(&mut b"\n{\"id\":1,\"result\":{}}\n".as_slice())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(frame.0["id"], 1);
    }

    /// Collects the reply text the way the driver's `ReplyBuf` does.
    #[derive(Clone, Default)]
    struct Recorder(Arc<Mutex<String>>);
    impl ClientHooks for Recorder {
        fn on_permission(&self, _tool: &Value) -> PermissionDecision {
            PermissionDecision::Allow
        }
        fn on_update(&self, update: &Value) {
            if let Some(text) = agent_message_text(update) {
                self.0.lock().unwrap().push_str(&text);
            }
        }
    }

    #[tokio::test]
    async fn a_reply_chunk_written_with_the_response_is_not_lost() {
        let dir = tempfile::tempdir().unwrap();
        let reply = Recorder::default();
        let mut acp = AcpSession::start(
            &fake_agent_command(),
            dir.path(),
            reply.clone(),
            None,
            "fake-acp:burst",
            "circle",
            None,
        )
        .await
        .unwrap();
        let never = CancellationToken::new();
        // Each turn is a coin flip without the fix, so take enough of them
        // that losing none by luck is not plausible.
        for turn in 0..32 {
            reply.0.lock().unwrap().clear();
            let result = tokio::time::timeout(LIVENESS, acp.prompt_until("ask", &never))
                .await
                .expect("the turn ends")
                .unwrap();
            assert_eq!(result.stop_reason, "end_turn");
            assert_eq!(*reply.0.lock().unwrap(), "the answer", "turn {turn}");
        }
        acp.shutdown().await;
    }

    async fn fake_session(mode: &str) -> (AcpSession<AllowAll>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let session = AcpSession::start(
            &fake_agent_command(),
            dir.path(),
            AllowAll,
            None,
            &format!("fake-acp:{mode}"),
            "circle",
            None,
        )
        .await
        .unwrap();
        (session, dir)
    }

    #[tokio::test]
    async fn stopping_a_turn_asks_the_agent_to_cancel_and_the_turn_ends_cancelled() {
        let (mut acp, _dir) = fake_session("cooperative").await;
        let activity = Arc::new(Mutex::new(Activity::default()));
        acp.set_activity(activity.clone());
        let stop = CancellationToken::new();
        // Stop only once the agent is visibly in its tool call: a quiet agent
        // with a tool call open is working, and is not stopped for being quiet.
        let stopper = {
            let (stop, activity) = (stop.clone(), activity.clone());
            tokio::spawn(async move {
                while activity.lock().unwrap().open_tools.is_empty() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                stop.cancel();
            })
        };

        let turn = tokio::time::timeout(LIVENESS, acp.prompt_until("build it", &stop))
            .await
            .expect("the turn ends once the agent answers the cancel")
            .unwrap();

        assert_eq!(turn.stop_reason, "cancelled");
        assert_eq!(
            activity.lock().unwrap().open_tools["t1"].title,
            "long build"
        );
        stopper.await.unwrap();
        acp.shutdown().await;
    }

    #[tokio::test]
    async fn an_agent_that_ignores_the_cancel_is_given_up_on_after_the_grace() {
        let (mut acp, _dir) = fake_session("stubborn").await;
        acp.cancel_grace = Duration::from_millis(200);
        let stop = CancellationToken::new();
        stop.cancel();

        let error = tokio::time::timeout(LIVENESS, acp.prompt_until("build it", &stop))
            .await
            .expect("the grace bounds how long a stop can take")
            .unwrap_err();

        assert!(error.to_string().contains("did not stop"), "{error}");
        acp.shutdown().await;
    }
}
