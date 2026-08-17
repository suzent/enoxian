use anyhow::{bail, Context, Result};
use std::fs::{self, OpenOptions};
use std::process::{Command, Stdio};

pub async fn run(port: Option<u16>) -> Result<()> {
    if crate::commands::service::is_installed() {
        if port.is_some() {
            bail!(
                "--port cannot override an installed service; reinstall it with \
                 `enox service install --force --port <PORT>`"
            );
        }
        return crate::commands::service::start();
    }

    let port = port.unwrap_or(36521);
    let enox = std::env::current_exe().context("failed to locate the enox executable")?;
    let log_dir = dirs::home_dir()
        .unwrap_or_else(|| ".".into())
        .join(".enoxian")
        .join("logs");
    fs::create_dir_all(&log_dir)
        .with_context(|| format!("failed to create {}", log_dir.display()))?;
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("daemon.log"))?;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("daemon.err.log"))?;

    #[cfg(unix)]
    {
        Command::new(&enox)
            .args(["daemon", "run", "--port", &port.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .with_context(|| format!("failed to start {}", enox.display()))?;
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        Command::new(&enox)
            .args(["daemon", "run", "--port", &port.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .spawn()
            .with_context(|| format!("failed to start {}", enox.display()))?;
    }

    println!("✓ Enoxian started on port {port}");
    println!("  logs: {}", log_dir.display());
    Ok(())
}
