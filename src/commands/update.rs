use anyhow::{bail, Result};
use std::path::PathBuf;
use std::process::Command;
use crate::config;

pub async fn run(dev: bool, src: Option<PathBuf>, no_pull: bool) -> Result<()> {
    if dev {
        run_dev(src, no_pull)
    } else {
        run_stable()
    }
}

fn run_dev(src: Option<PathBuf>, no_pull: bool) -> Result<()> {
    let src = resolve_src(src)?;

    if !no_pull {
        println!("▶ Pulling latest...");
        let status = Command::new("git")
            .args(["-C", &src.to_string_lossy(), "pull"])
            .status()?;
        if !status.success() {
            bail!("git pull failed");
        }
    }

    println!("▶ Installing binaries to ~/.cargo/bin/ ...");
    let status = Command::new("cargo")
        .args(["install", "--path", &src.to_string_lossy(), "--bins"])
        .status()?;
    if !status.success() {
        bail!("cargo install failed");
    }

    restart_daemon()?;
    println!("✓ Update complete");
    Ok(())
}

fn run_stable() -> Result<()> {
    // M12: download pre-built binary from GitHub Releases.
    // Until then, guide users to --dev.
    println!("Stable binary downloads are not yet available (coming in M12).");
    println!("To update from source: enoch update --dev [--src <path>]");
    Ok(())
}

/// Resolve source dir: prefer explicit arg, then saved config, then error.
fn resolve_src(arg: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = arg {
        if !p.join("Cargo.toml").exists() {
            bail!("'{}' doesn't look like an enochian source directory", p.display());
        }
        // Save for future use
        let mut cfg = config::load_global();
        cfg.dev_src = Some(p.to_string_lossy().into_owned());
        let _ = config::save_global(&cfg);
        return Ok(p);
    }

    let cfg = config::load_global();
    if let Some(saved) = cfg.dev_src {
        let p = PathBuf::from(&saved);
        if p.join("Cargo.toml").exists() {
            return Ok(p);
        }
        bail!("saved source path '{}' no longer exists — run with --src <path>", saved);
    }

    bail!("no source path configured — run once with: enoch update --dev --src <path/to/enochian>")
}

fn restart_daemon() -> Result<()> {
    println!("▶ Restarting enochd...");
    #[cfg(unix)]
    {
        Command::new("pkill").args(["-f", "enochd"]).status().ok();
        std::thread::sleep(std::time::Duration::from_secs(1));
        Command::new("enochd")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        Command::new("taskkill").args(["/F", "/IM", "enochd.exe"]).status().ok();
        std::thread::sleep(std::time::Duration::from_secs(1));
        Command::new("enochd.exe")
            .creation_flags(0x00000008) // DETACHED_PROCESS
            .spawn()?;
    }
    println!("✓ enochd restarted");
    Ok(())
}
