use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub async fn run(port: u16) -> Result<()> {
    let enochd = find_enochd()?;

    #[cfg(unix)]
    {
        Command::new(&enochd)
            .arg("--port").arg(port.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("failed to start {}", enochd.display()))?;
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        Command::new(&enochd)
            .arg("--port").arg(port.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x00000010) // CREATE_NEW_CONSOLE
            .spawn()
            .with_context(|| format!("failed to start {}", enochd.display()))?;
    }

    println!("✓ enochd started on port {port}");
    Ok(())
}

fn find_enochd() -> Result<PathBuf> {
    let exe_name = if cfg!(windows) { "enochd.exe" } else { "enochd" };

    // Prefer sibling of current executable (works for both cargo install and dev builds).
    if let Ok(current) = std::env::current_exe() {
        let sibling = current.parent().unwrap_or(current.as_path()).join(exe_name);
        if sibling.exists() {
            return Ok(sibling);
        }
    }

    // Fall back to ~/.cargo/bin/enochd
    if let Some(home) = dirs::home_dir() {
        let p = home.join(".cargo").join("bin").join(exe_name);
        if p.exists() {
            return Ok(p);
        }
    }

    anyhow::bail!("enochd not found — run `cargo install --path .` to install it")
}
