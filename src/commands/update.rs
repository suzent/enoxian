use crate::config;
use anyhow::{bail, Result};
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

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
fn install_unix(src: &Path) -> Result<()> {
    println!("▶ Stopping Enoxian...");
    if let Ok(exe) = std::env::current_exe() {
        let _ = Command::new(exe).arg("stop").status();
    }
    println!("▶ Installing enox to ~/.cargo/bin/ ...");
    // On Unix, running executables can be replaced in-place (inode swap).
    let status = Command::new("cargo")
        .args(["install", "--path", &src.to_string_lossy(), "--bins"])
        .status()?;
    if !status.success() {
        bail!("cargo install failed");
    }
    restart_service()?;
    println!("✓ Update complete");
    Ok(())
}

#[cfg(windows)]
fn install_windows(src: &PathBuf) -> Result<()> {
    use std::os::windows::process::CommandExt;

    println!("▶ Building release binary...");
    let status = Command::new("cargo")
        .args(["build", "--release", "--bins"])
        .current_dir(src)
        .status()?;
    if !status.success() {
        bail!("cargo build failed");
    }

    // Ask the unified daemon process to stop. The current CLI remains locked
    // until this update command exits, so replacement is deferred below.
    Command::new(std::env::current_exe()?)
        .arg("stop")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok();

    // Write a PowerShell script to replace enox and restart it after this CLI exits.
    let cargo_bin = home_cargo_bin()?;
    let enox_src = src.join("target\\release\\enox.exe");
    let enox_dst = cargo_bin.join("enox.exe");
    let legacy_enoxd = cargo_bin.join("enoxd.exe");
    let script_path = std::env::temp_dir().join("enox-update.ps1");

    let script = format!(
        "Start-Sleep -Seconds 2\n\
         $log = \"$env:TEMP\\enox-update.log\"\n\
         \"$(Get-Date): copying enox\" | Out-File $log\n\
         Copy-Item -Force '{enox_src}' '{enox_dst}'\n\
         Remove-Item -Force '{legacy_enoxd}' -ErrorAction SilentlyContinue\n\
         \"$(Get-Date): starting Enoxian\" | Out-File $log -Append\n\
         Start-Process -FilePath '{enox_dst}' -ArgumentList 'start' -WindowStyle Hidden\n\
         \"$(Get-Date): done\" | Out-File $log -Append\n\
         Remove-Item $MyInvocation.MyCommand.Path -ErrorAction SilentlyContinue\n",
        enox_src = enox_src.display(),
        enox_dst = enox_dst.display(),
        legacy_enoxd = legacy_enoxd.display(),
    );
    std::fs::write(&script_path, script)?;

    // Spawn PowerShell in a hidden console; it runs after this process exits.
    Command::new("powershell")
        .args([
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-File",
            &script_path.to_string_lossy(),
        ])
        .creation_flags(0x00000010) // CREATE_NEW_CONSOLE
        .spawn()?;

    println!("✓ Binary built. Replacement will be applied after this process exits.");
    println!("  Enoxian will restart automatically.");
    Ok(())
}

fn run_stable() -> Result<()> {
    println!("Stable installs are updated by rerunning the release installer.");
    println!("Download: https://github.com/suzent/enoxian/releases/latest");
    println!("Development source: enox update --dev [--src <path>]");
    Ok(())
}

fn resolve_src(arg: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = arg {
        if !p.join("Cargo.toml").exists() {
            bail!(
                "'{}' doesn't look like an enoxian source directory",
                p.display()
            );
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
        bail!("saved source path '{saved}' no longer exists — run with --src <path>");
    }

    if let Ok(saved) = std::env::var("ENOXIAN_SRC") {
        let p = PathBuf::from(&saved);
        if p.join("Cargo.toml").exists() {
            return Ok(p);
        }
        bail!("ENOXIAN_SRC '{saved}' does not look like an enoxian source directory");
    }

    bail!("no source path configured — run once with: enox update --dev --src <path/to/enoxian>")
}

#[cfg(unix)]
fn restart_service() -> Result<()> {
    println!("▶ Restarting Enoxian...");
    let status = Command::new("enox").arg("start").status()?;
    if !status.success() {
        bail!("failed to restart Enoxian");
    }
    println!("✓ Enoxian restarted");
    Ok(())
}

#[cfg(windows)]
fn home_cargo_bin() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot resolve home directory"))?;
    Ok(home.join(".cargo").join("bin"))
}
