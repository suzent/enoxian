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

    #[cfg(unix)]
    install_unix(&src)?;

    #[cfg(windows)]
    install_windows(&src)?;

    Ok(())
}

#[cfg(unix)]
fn install_unix(src: &PathBuf) -> Result<()> {
    println!("▶ Installing binaries to ~/.cargo/bin/ ...");
    // On Unix, running executables can be replaced in-place (inode swap).
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

#[cfg(windows)]
fn install_windows(src: &PathBuf) -> Result<()> {
    use std::os::windows::process::CommandExt;

    println!("▶ Building release binaries...");
    let status = Command::new("cargo")
        .args(["build", "--release", "--bins"])
        .current_dir(src)
        .status()?;
    if !status.success() {
        bail!("cargo build failed");
    }

    // Kill enochd now (not locked). enoch.exe itself is still locked until we exit.
    // Suppress output — "process not found" is expected if daemon wasn't running.
    Command::new("taskkill")
        .args(["/F", "/IM", "enochd.exe"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok();

    // Write a PowerShell script to copy binaries + restart daemon after enoch.exe exits.
    let cargo_bin = home_cargo_bin()?;
    let enoch_src  = src.join("target\\release\\enoch.exe");
    let enochd_src = src.join("target\\release\\enochd.exe");
    let enoch_dst  = cargo_bin.join("enoch.exe");
    let enochd_dst = cargo_bin.join("enochd.exe");
    let script_path = std::env::temp_dir().join("enoch-update.ps1");

    let script = format!(
        "Start-Sleep -Seconds 2\n\
         $log = \"$env:TEMP\\enoch-update.log\"\n\
         \"$(Get-Date): copying binaries\" | Out-File $log\n\
         Copy-Item -Force '{enoch_src}' '{enoch_dst}'\n\
         Copy-Item -Force '{enochd_src}' '{enochd_dst}'\n\
         \"$(Get-Date): starting enochd\" | Out-File $log -Append\n\
         Start-Process -FilePath '{enochd_dst}' -WindowStyle Hidden\n\
         \"$(Get-Date): done\" | Out-File $log -Append\n\
         Remove-Item $MyInvocation.MyCommand.Path -ErrorAction SilentlyContinue\n",
        enoch_src  = enoch_src.display(),
        enochd_src = enochd_src.display(),
        enoch_dst  = enoch_dst.display(),
        enochd_dst = enochd_dst.display(),
    );
    std::fs::write(&script_path, script)?;

    // Spawn PowerShell in a new hidden console — runs after this process exits.
    // CREATE_NEW_CONSOLE (0x10) gives PowerShell its own console so it can run properly.
    // -WindowStyle Hidden keeps it invisible.
    Command::new("powershell")
        .args([
            "-NonInteractive",
            "-WindowStyle", "Hidden",
            "-File", &script_path.to_string_lossy(),
        ])
        .creation_flags(0x00000010) // CREATE_NEW_CONSOLE
        .spawn()?;

    println!("✓ Binaries built. Replacements will be applied in 2 seconds after this process exits.");
    println!("  enochd will restart automatically.");
    Ok(())
}

fn run_stable() -> Result<()> {
    println!("Stable binary downloads are not yet available (coming in M12).");
    println!("To update from source: enoch update --dev [--src <path>]");
    Ok(())
}

fn resolve_src(arg: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = arg {
        if !p.join("Cargo.toml").exists() {
            bail!("'{}' doesn't look like an enochian source directory", p.display());
        }
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

#[cfg(unix)]
fn restart_daemon() -> Result<()> {
    println!("▶ Restarting enochd...");
    Command::new("pkill").args(["-f", "enochd"]).status().ok();
    std::thread::sleep(std::time::Duration::from_secs(1));
    Command::new("enochd")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    println!("✓ enochd restarted");
    Ok(())
}

#[cfg(windows)]
fn home_cargo_bin() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot resolve home directory"))?;
    Ok(home.join(".cargo").join("bin"))
}
