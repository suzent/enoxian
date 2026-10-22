//! Visible terminal authentication with a supervised, private launch handoff.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, SystemTime},
};

#[derive(Serialize, Deserialize)]
struct Launch {
    command: Vec<String>,
    cwd: PathBuf,
    env: BTreeMap<String, String>,
}

pub fn available() -> bool {
    if cfg!(windows) || cfg!(target_os = "macos") {
        return true;
    }
    (std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some())
        && linux_terminal().is_some()
}
fn linux_terminal() -> Option<(&'static str, &'static str)> {
    [
        ("x-terminal-emulator", "-e"),
        ("gnome-terminal", "--"),
        ("konsole", "-e"),
        ("xterm", "-e"),
    ]
    .into_iter()
    .find(|(name, _)| super::probe::resolve(name).is_some())
}

struct Handoff(PathBuf);
impl Drop for Handoff {
    fn drop(&mut self) {
        // The worker owns child cleanup; a dropped job stops renewing its heartbeat too.
        let _ = std::fs::write(self.0.join("cancel"), []);
        let _ = std::fs::remove_file(self.0.join("launch.json"));
    }
}
fn private_json(path: &Path, value: &Launch) -> Result<()> {
    crate::config::write_secret(path, serde_json::to_vec(value)?)
}

pub async fn run(command: Vec<String>, cwd: PathBuf, env: BTreeMap<String, String>) -> Result<()> {
    if !available() {
        bail!("No interactive terminal is available on this device");
    }
    let directory = crate::config::enoxian_dir()?
        .join("auth-jobs")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    }
    let guard = Handoff(directory.clone());
    private_json(
        &directory.join("launch.json"),
        &Launch { command, cwd, env },
    )?;
    std::fs::write(directory.join("heartbeat"), [])?;
    let exe = std::env::current_exe()?;
    let mut launcher;
    #[cfg(windows)]
    {
        launcher = tokio::process::Command::new(&exe);
        launcher
            .args(["acp-auth-worker"])
            .arg(&directory)
            .creation_flags(0x00000010);
    }
    #[cfg(target_os = "macos")]
    {
        let quoted = |s: &str| format!("'{}'", s.replace('\'', r"'\''"));
        let command = format!(
            "{} acp-auth-worker {}",
            quoted(&exe.to_string_lossy()),
            quoted(&directory.to_string_lossy())
        );
        launcher = tokio::process::Command::new("osascript");
        launcher.args(["-e", "on run argv\ntell application \"Terminal\"\nactivate\ndo script (item 1 of argv)\nend tell\nend run", &command]);
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let (terminal, argument) = linux_terminal().context("No terminal")?;
        launcher = tokio::process::Command::new(terminal);
        launcher
            .arg(argument)
            .arg(&exe)
            .arg("acp-auth-worker")
            .arg(&directory);
    }
    let mut child = match launcher.spawn() {
        Ok(child) => child,
        Err(error) => {
            drop(guard);
            let _ = std::fs::remove_dir_all(&directory);
            return Err(error.into());
        }
    };
    let started = tokio::time::Instant::now();
    let deadline = started + Duration::from_secs(300);
    loop {
        if let Ok(result) = std::fs::read_to_string(directory.join("exit")) {
            drop(guard);
            let _ = std::fs::remove_dir_all(&directory);
            if result == "0" {
                return Ok(());
            }
            bail!("Login terminal exited without success");
        }
        if started.elapsed() > Duration::from_secs(15) && !directory.join("started").exists() {
            bail!("The login terminal did not start");
        }
        if let Ok(metadata) = std::fs::metadata(directory.join("started")) {
            if metadata.modified()?.elapsed().unwrap_or_default() > Duration::from_secs(5) {
                bail!("The login terminal closed before completing authentication");
            }
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("Terminal login timed out");
        }
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                bail!("Could not open login terminal");
            }
        }
        std::fs::write(directory.join("heartbeat"), [])?;
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

pub(super) struct ProcessTree(pub(super) u32);
impl Drop for ProcessTree {
    fn drop(&mut self) {
        super::spawn::kill_tree(self.0);
    }
}

/// Internal CLI entry point. Never accepts a command from an HTTP request.
pub async fn worker(directory: &Path) -> Result<()> {
    let result = worker_inner(directory).await;
    if result.is_err() {
        let _ = std::fs::remove_file(directory.join("launch.json"));
        let _ = std::fs::write(directory.join("exit.tmp"), "1");
        let _ = std::fs::rename(directory.join("exit.tmp"), directory.join("exit"));
    }
    result
}

async fn worker_inner(directory: &Path) -> Result<()> {
    let config = directory.join("launch.json");
    let launch: Launch = serde_json::from_slice(&std::fs::read(&config)?)?;
    std::fs::remove_file(config)?;
    if directory.join("cancel").exists() {
        return Ok(());
    }
    std::fs::write(directory.join("started"), [])?;
    let (program, args) = launch
        .command
        .split_first()
        .context("Empty login command")?;
    let mut command = super::spawn::command(program, args);
    command
        .current_dir(&launch.cwd)
        .envs(&launch.env)
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        let input = std::fs::OpenOptions::new().read(true).open("CONIN$")?;
        let output = std::fs::OpenOptions::new().write(true).open("CONOUT$")?;
        command
            .stdin(Stdio::from(input))
            .stdout(Stdio::from(output.try_clone()?))
            .stderr(Stdio::from(output));
    }
    #[cfg(not(windows))]
    {
        command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
    }
    let mut child = command.spawn()?;
    let tree = ProcessTree(child.id().context("Missing login process")?);
    let started = tokio::time::Instant::now();
    let exit = loop {
        if let Some(status) = child.try_wait()? {
            break status.code().unwrap_or(1);
        }
        let fresh = std::fs::metadata(directory.join("heartbeat"))
            .and_then(|m| m.modified())
            .ok()
            .and_then(|time| SystemTime::now().duration_since(time).ok())
            .is_some_and(|age| age < Duration::from_secs(5));
        if !fresh
            || directory.join("cancel").exists()
            || started.elapsed() > Duration::from_secs(300)
        {
            drop(tree);
            let _ = child.wait().await;
            let _ = std::fs::remove_dir_all(directory);
            return Ok(());
        }
        std::fs::write(directory.join("started"), [])?;
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    // All descendants belong to this completed login, including helper processes.
    drop(tree);
    std::fs::write(directory.join("exit.tmp"), exit.to_string())?;
    std::fs::rename(directory.join("exit.tmp"), directory.join("exit"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelling_a_handoff_removes_launch_secrets() {
        let directory = tempfile::tempdir().unwrap();
        private_json(
            &directory.path().join("launch.json"),
            &Launch {
                command: vec!["agent".into()],
                cwd: directory.path().into(),
                env: BTreeMap::from([("KEY".into(), "secret".into())]),
            },
        )
        .unwrap();
        drop(Handoff(directory.path().into()));
        assert!(!directory.path().join("launch.json").exists());
        assert!(directory.path().join("cancel").exists());
    }

    #[tokio::test]
    async fn a_worker_start_failure_publishes_failure_and_removes_handoff() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("launch.json"), "invalid").unwrap();
        assert!(worker(directory.path()).await.is_err());
        assert!(!directory.path().join("launch.json").exists());
        assert_eq!(
            std::fs::read_to_string(directory.path().join("exit")).unwrap(),
            "1"
        );
    }
}
