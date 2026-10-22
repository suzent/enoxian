//! Operator-requested ACP sign-in. No prompts, credentials, or raw provider output enter the API.
use super::{
    acp::{self, AcpSession, ClientHooks, PermissionDecision},
    config::{AgentCommand, AgentConfig, Driver},
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Unknown,
    Checking,
    AuthRequired,
    Authenticating,
    Terminal,
    Verifying,
    Ready,
    Failed,
    Cancelled,
}
impl Status {
    pub fn active(self) -> bool {
        matches!(
            self,
            Self::Checking | Self::Authenticating | Self::Terminal | Self::Verifying
        )
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Method {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub available: bool,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct AuthState {
    pub status: Status,
    pub methods: Vec<Method>,
    pub error: Option<String>,
}
struct Job {
    state: AuthState,
    stop: CancellationToken,
}
#[derive(Clone, Default)]
pub struct AuthJobs(Arc<Mutex<HashMap<String, Job>>>);
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Probe,
    Login,
}

impl AuthJobs {
    pub fn states(&self) -> HashMap<String, AuthState> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|(id, job)| (id.clone(), job.state.clone()))
            .collect()
    }
    pub fn cancel(&self, id: &str) {
        if let Some(job) = self.0.lock().unwrap().get(id) {
            job.stop.cancel();
        }
    }
    fn update(&self, id: &str, status: Status) {
        if let Some(job) = self.0.lock().unwrap().get_mut(id) {
            job.state.status = status;
        }
    }
    pub fn start(
        &self,
        id: &str,
        action: Action,
        method: Option<String>,
        shutdown: CancellationToken,
        workspace: Option<&std::path::Path>,
    ) -> Result<AuthState> {
        let cfg = AgentConfig::load();
        let mut cmd = cfg
            .agents
            .get(id)
            .context("Agent is not configured")?
            .clone();
        if cmd.driver != Driver::Acp || super::plugin::command_status(&cmd.command) != "ready" {
            bail!("An installed ACP agent is required");
        }
        if let Some(workspace) = workspace {
            cmd.working_dir = Some(
                super::driver::working_dir(workspace, cmd.working_dir.as_deref())
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        self.start_command(id, cmd, action, method, shutdown)
    }

    fn start_command(
        &self,
        id: &str,
        cmd: AgentCommand,
        action: Action,
        method: Option<String>,
        shutdown: CancellationToken,
    ) -> Result<AuthState> {
        let mut jobs = self.0.lock().unwrap();
        if let Some(job) = jobs.get(id).filter(|job| job.state.status.active()) {
            return Ok(job.state.clone());
        }
        if jobs
            .values()
            .filter(|job| job.state.status.active())
            .count()
            >= 4
        {
            bail!("Too many active checks or sign-ins");
        }
        let stop = shutdown.child_token();
        let state = AuthState {
            status: Status::Checking,
            ..Default::default()
        };
        jobs.insert(
            id.to_owned(),
            Job {
                state: state.clone(),
                stop: stop.clone(),
            },
        );
        let this = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            let outcome = tokio::select! {
                _ = stop.cancelled() => (Status::Cancelled, None),
                result = tokio::time::timeout(Duration::from_secs(300), this.run(&id, &cmd, action, method)) => {
                    match result {
                        Ok(Ok(status)) => (status, None),
                        Ok(Err(error)) if acp::auth_required(&error) => (Status::AuthRequired, None),
                        Ok(Err(_)) => (Status::Failed, Some("Could not verify sign-in. Check the agent installation and try again.".into())),
                        Err(_) => (Status::Failed, Some("Sign-in or connection check timed out after five minutes.".into())),
                    }
                }
            };
            if let Some(job) = this.0.lock().unwrap().get_mut(&id) {
                job.state.status = outcome.0;
                job.state.error = outcome.1;
            }
        });
        Ok(state)
    }

    async fn run(
        &self,
        id: &str,
        cmd: &AgentCommand,
        action: Action,
        method: Option<String>,
    ) -> Result<Status> {
        let base = crate::config::enoxian_dir()?.join("auth-workspace");
        let cwd = super::driver::working_dir(&base, cmd.working_dir.as_deref());
        tokio::fs::create_dir_all(&cwd).await?;
        let mut session = AcpSession::connect(
            &cmd.command,
            &cwd,
            Deny,
            id,
            "",
            None,
            super::auth_terminal::available(),
        )
        .await?;
        let raw_methods = session.auth_methods.clone();
        let methods = raw_methods
            .iter()
            .filter_map(|raw| {
                let id = raw.get("id")?.as_str()?.to_owned();
                let kind = raw
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("agent")
                    .to_owned();
                Some(Method {
                    name: raw
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(&id)
                        .to_owned(),
                    id,
                    available: kind == "agent"
                        || (kind == "terminal" && super::auth_terminal::available()),
                    kind,
                })
            })
            .collect();
        if let Some(job) = self.0.lock().unwrap().get_mut(id) {
            job.state.methods = methods;
        }
        if matches!(action, Action::Login) {
            let selected = if let Some(method) = method {
                Some(
                    raw_methods
                        .iter()
                        .find(|raw| raw["id"].as_str() == Some(&method))
                        .context("Unknown authentication method")?,
                )
            } else {
                ["agent", "terminal"].iter().find_map(|kind| {
                    raw_methods.iter().find(|raw| {
                        raw.get("type").and_then(Value::as_str).unwrap_or("agent") == *kind
                    })
                })
            };
            match selected.map(|raw| raw.get("type").and_then(Value::as_str).unwrap_or("agent")) {
                Some("agent") => {
                    self.update(id, Status::Authenticating);
                    session
                        .authenticate(
                            selected.unwrap()["id"]
                                .as_str()
                                .context("Missing method ID")?,
                        )
                        .await?;
                }
                Some("terminal") => {
                    let selected = selected.unwrap();
                    let args: Vec<String> = serde_json::from_value(
                        selected
                            .get("args")
                            .cloned()
                            .unwrap_or(serde_json::json!([])),
                    )?;
                    let env = serde_json::from_value(
                        selected
                            .get("env")
                            .cloned()
                            .unwrap_or(serde_json::json!({})),
                    )?;
                    let mut command = cmd.command.clone();
                    command.extend(args);
                    session.shutdown().await;
                    self.update(id, Status::Terminal);
                    super::auth_terminal::run(command, cwd.clone(), env).await?;
                    session = AcpSession::connect(
                        &cmd.command,
                        &cwd,
                        Deny,
                        id,
                        "",
                        None,
                        super::auth_terminal::available(),
                    )
                    .await?;
                }
                None => {
                    let bridge = super::probe::bridged_cli(&cmd.command[0])
                        .context("No supported login method")?;
                    let command = bridge
                        .login_command
                        .split_ascii_whitespace()
                        .map(str::to_owned)
                        .collect();
                    session.shutdown().await;
                    self.update(id, Status::Terminal);
                    super::auth_terminal::run(command, cwd.clone(), Default::default()).await?;
                    session = AcpSession::connect(
                        &cmd.command,
                        &cwd,
                        Deny,
                        id,
                        "",
                        None,
                        super::auth_terminal::available(),
                    )
                    .await?;
                }
                _ => bail!("Unsupported authentication method"),
            }
            self.update(id, Status::Verifying);
        }
        session.new_session().await?;
        session.shutdown().await;
        // A fresh connection must work too: task runs use their own process and actor identity.
        if matches!(action, Action::Login) {
            let mut fresh =
                AcpSession::connect(&cmd.command, &cwd, Deny, id, "", None, false).await?;
            fresh.new_session().await?;
            fresh.shutdown().await;
        }
        if let Some(bridge) = super::probe::bridged_cli(&cmd.command[0]) {
            if let Some(args) = bridge.auth_status_args {
                let args = args.iter().map(|s| s.to_string()).collect::<Vec<_>>();
                let mut child = super::spawn::command(bridge.program, &args)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .kill_on_drop(true)
                    .spawn()?;
                let _tree = super::auth_terminal::ProcessTree(
                    child.id().context("Missing status process")?,
                );
                let result = tokio::time::timeout(Duration::from_secs(15), child.wait()).await;
                if result.is_err() {
                    if let Some(pid) = child.id() {
                        super::spawn::kill_tree(pid);
                    }
                }
                match result??.code() {
                    Some(0) => {}
                    Some(1) => return Ok(Status::AuthRequired),
                    _ => bail!("CLI authentication check failed"),
                }
            }
        }
        Ok(Status::Ready)
    }
}
struct Deny;
impl ClientHooks for Deny {
    fn on_permission(&self, _: &Value) -> PermissionDecision {
        PermissionDecision::Deny
    }
    fn write_file(&self, _: &std::path::Path, _: &[u8]) -> Result<()> {
        bail!("File writes are disabled during login")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authentication_resolves_relative_absolute_and_default_directories() {
        let base = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let resolve = super::super::driver::working_dir;
        assert_eq!(resolve(base.path(), Some("src")), base.path().join("src"));
        assert_eq!(resolve(base.path(), Some("")), base.path());
        assert_eq!(resolve(base.path(), None), base.path());
        assert_eq!(
            resolve(base.path(), Some(other.path().to_str().unwrap())),
            other.path()
        );
    }

    fn command(dir: &std::path::Path) -> AgentCommand {
        AgentCommand {
            command: super::super::acp::tests::fake_agent_command(),
            driver: Driver::Acp,
            working_dir: Some(dir.to_string_lossy().into_owned()),
            ..Default::default()
        }
    }
    async fn finished(jobs: &AuthJobs, id: &str) -> AuthState {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let state = jobs.states()[id].clone();
                if !state.status.active() {
                    return state;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }
    #[tokio::test]
    async fn jobs_report_auth_required_and_do_not_misreport_connection_only_login() {
        let dir = tempfile::tempdir().unwrap();
        let jobs = AuthJobs::default();
        jobs.start_command(
            "fake-acp:auth",
            command(dir.path()),
            Action::Probe,
            None,
            CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(
            finished(&jobs, "fake-acp:auth").await.status,
            Status::AuthRequired
        );
        jobs.start_command(
            "fake-acp:auth",
            command(dir.path()),
            Action::Login,
            Some("login".into()),
            CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(
            finished(&jobs, "fake-acp:auth").await.status,
            Status::AuthRequired
        );
    }
    #[tokio::test]
    async fn cancellation_and_daemon_shutdown_end_pending_handshakes() {
        let dir = tempfile::tempdir().unwrap();
        let jobs = AuthJobs::default();
        let shutdown = CancellationToken::new();
        jobs.start_command(
            "fake-acp:mute",
            command(dir.path()),
            Action::Probe,
            None,
            shutdown.clone(),
        )
        .unwrap();
        jobs.cancel("fake-acp:mute");
        assert_eq!(
            finished(&jobs, "fake-acp:mute").await.status,
            Status::Cancelled
        );
        jobs.start_command(
            "fake-acp:mute",
            command(dir.path()),
            Action::Probe,
            None,
            shutdown.clone(),
        )
        .unwrap();
        shutdown.cancel();
        assert_eq!(
            finished(&jobs, "fake-acp:mute").await.status,
            Status::Cancelled
        );
    }
}
