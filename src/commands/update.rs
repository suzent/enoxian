use crate::{cli::UpdateApplyArgs, config};
use anyhow::{bail, Context, Result};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

const CHANNEL_DEV: &str = "dev";
const CHANNEL_STABLE: &str = "stable";

pub async fn run(
    dev: bool,
    src: Option<PathBuf>,
    no_pull: bool,
    status: bool,
    record_stable: bool,
) -> Result<()> {
    if record_stable {
        return record_stable_install();
    }
    if status {
        return show_status();
    }

    let cfg = config::load_global();
    if dev || cfg.update_channel.as_deref() == Some(CHANNEL_DEV) {
        run_dev(src, no_pull)
    } else {
        show_stable_guidance();
        Ok(())
    }
}

fn run_dev(src: Option<PathBuf>, no_pull: bool) -> Result<()> {
    let src = resolve_src(src)?;

    if !no_pull {
        println!("▶ Pulling latest source...");
        let status = Command::new("git")
            .args(["-C", &src.to_string_lossy(), "pull", "--ff-only"])
            .status()?;
        if !status.success() {
            bail!("git pull --ff-only failed; resolve local branch changes or use --no-pull");
        }
    }

    println!("▶ Building development binary...");
    let status = Command::new("cargo")
        .args(["build", "--release", "--bin", "enox"])
        .current_dir(&src)
        .status()?;
    if !status.success() {
        bail!("cargo build failed; the current installation was not changed");
    }

    let source = release_binary(&src);
    verify_binary(&source).context("new development binary failed its pre-install check")?;
    let service = crate::commands::service::is_installed();
    let target = managed_target(service)?;

    println!("▶ Stopping Enoxian...");
    stop_current(service)?;

    #[cfg(windows)]
    {
        spawn_windows_apply(source, target, src, service)?;
        println!("▶ Handed off to the verified development binary...");
        println!("  It will replace this executable, restart Enoxian, and verify API health.");
        println!("  Progress: ~/.enoxian/logs/update.log");
        Ok(())
    }

    #[cfg(not(windows))]
    {
        apply(UpdateApplyArgs {
            source,
            target,
            service,
            dev_source: src,
        })
    }
}

pub fn apply(args: UpdateApplyArgs) -> Result<()> {
    #[cfg(windows)]
    thread::sleep(Duration::from_millis(750));

    let backup = backup_path(&args.target)?;
    let staged = staged_path(&args.target)?;
    let had_target = args.target.is_file();

    if same_path(&args.source, &args.target) {
        println!("▶ Development binary is already at the managed path.");
    } else {
        println!(
            "▶ Installing development binary to {}...",
            args.target.display()
        );
        if let Some(parent) = args.target.parent() {
            fs::create_dir_all(parent)?;
        }
        let _ = fs::remove_file(&backup);
        let _ = fs::remove_file(&staged);
        if had_target {
            fs::copy(&args.target, &backup)
                .with_context(|| format!("failed to back up {}", args.target.display()))?;
        }
        fs::copy(&args.source, &staged)
            .with_context(|| format!("failed to stage {}", args.source.display()))?;
        replace_with_retry(&staged, &args.target)?;
    }

    if let Err(error) = verify_binary(&args.target)
        .and_then(|_| start_target(&args.target, args.service))
        .and_then(|_| wait_for_health(&args.target))
    {
        eprintln!("✗ Development update failed: {error:#}");
        if !same_path(&args.source, &args.target) {
            rollback(&args.target, &backup, had_target, args.service)?;
        }
        bail!("development update rolled back; the previous installation was restored");
    }

    let mut cfg = config::load_global();
    cfg.dev_src = Some(args.dev_source.to_string_lossy().into_owned());
    cfg.update_channel = Some(CHANNEL_DEV.to_string());
    cfg.managed_executable = Some(args.target.to_string_lossy().into_owned());
    config::save_global(&cfg)?;

    let _ = fs::remove_file(&backup);
    let _ = fs::remove_file(&staged);
    cleanup_alternate_dev_binary(&args.target);
    println!("✓ Development update installed and healthy");
    println!("  binary: {}", args.target.display());
    println!("  source: {}", args.dev_source.display());
    Ok(())
}

fn show_status() -> Result<()> {
    let cfg = config::load_global();
    let service = crate::commands::service::is_installed();
    let target = managed_target(service)?;
    let channel = cfg.update_channel.as_deref().unwrap_or(CHANNEL_STABLE);
    let version = command_output(&target, &["--version"]).unwrap_or_else(|| "unavailable".into());
    let healthy = command_succeeds(&target, &["circles", "--json"]);

    println!("channel: {channel}");
    println!("version: {version}");
    println!("binary: {}", target.display());
    let service_status = match (service, healthy) {
        (true, true) => "running",
        (true, false) => "installed (stopped)",
        (false, true) => "unmanaged (running)",
        (false, false) => "not installed",
    };
    println!("service: {service_status}");
    if let Some(source) = cfg.dev_src {
        println!("source: {source}");
    }
    Ok(())
}

fn record_stable_install() -> Result<()> {
    let exe = std::env::current_exe().context("failed to locate installed enox")?;
    let mut cfg = config::load_global();
    cfg.update_channel = Some(CHANNEL_STABLE.to_string());
    cfg.managed_executable = Some(exe.to_string_lossy().into_owned());
    config::save_global(&cfg)
}

fn show_stable_guidance() {
    println!("Stable installs are updated by rerunning the verified release installer.");
    println!("Download: https://github.com/suzent/enoxian/releases/latest");
    println!("Development source: enox update --dev [--src <path>]");
    println!("Current channel: enox update --status");
}

fn managed_target(service: bool) -> Result<PathBuf> {
    if service {
        if let Some(path) = crate::commands::service::installed_executable() {
            return Ok(path);
        }
    }
    let cfg = config::load_global();
    if let Some(path) = cfg.managed_executable.filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    std::env::current_exe().context("failed to locate the current enox executable")
}

fn release_binary(src: &Path) -> PathBuf {
    let name = if cfg!(windows) { "enox.exe" } else { "enox" };
    src.join("target").join("release").join(name)
}

fn verify_binary(path: &Path) -> Result<()> {
    let status = Command::new(path)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("failed to execute {}", path.display()))?;
    if !status.success() {
        bail!("{} --version failed", path.display());
    }
    Ok(())
}

fn stop_current(service: bool) -> Result<()> {
    let exe = std::env::current_exe().context("failed to locate the current enox executable")?;
    let mut command = Command::new(exe);
    if service {
        command.args(["service", "stop"]);
    } else {
        command.arg("stop");
    }
    let status = command.status().context("failed to stop Enoxian")?;
    if !status.success() {
        bail!("failed to stop Enoxian; the current installation was not changed");
    }
    Ok(())
}

fn start_target(target: &Path, service: bool) -> Result<()> {
    println!("▶ Restarting Enoxian...");
    let mut command = Command::new(target);
    if service {
        command.args(["service", "start"]);
    } else {
        command.arg("start");
    }
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let status = command.status()?;
    if !status.success() {
        bail!("failed to restart Enoxian");
    }
    println!("✓ Enoxian restarted");
    Ok(())
}

fn wait_for_health(target: &Path) -> Result<()> {
    println!("▶ Waiting for API health...");
    for _ in 0..40 {
        let healthy = Command::new(target)
            .args(["circles", "--json"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if healthy {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(500));
    }
    bail!("Enoxian API did not become healthy within 20 seconds")
}

fn replace_with_retry(staged: &Path, target: &Path) -> Result<()> {
    let mut last_error = None;
    for _ in 0..40 {
        match replace_once(staged, target) {
            Ok(()) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err(last_error.expect("replacement loop always attempts at least once"))
        .with_context(|| format!("failed to replace {}", target.display()))
}

fn replace_once(staged: &Path, target: &Path) -> std::io::Result<()> {
    if target.exists() {
        fs::remove_file(target)?;
    }
    fs::rename(staged, target)
}

fn rollback(target: &Path, backup: &Path, had_target: bool, service: bool) -> Result<()> {
    eprintln!("▶ Restoring previous installation...");
    stop_path(target, service);
    let _ = fs::remove_file(target);
    if had_target && backup.is_file() {
        fs::copy(backup, target)?;
        start_target(target, service)?;
        wait_for_health(target)?;
    }
    Ok(())
}

fn stop_path(target: &Path, service: bool) {
    if !target.is_file() {
        return;
    }
    let mut command = Command::new(target);
    if service {
        command.args(["service", "stop"]);
    } else {
        command.arg("stop");
    }
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let _ = command.status();
}

fn backup_path(target: &Path) -> Result<PathBuf> {
    adjacent_path(target, ".update-backup")
}

fn staged_path(target: &Path) -> Result<PathBuf> {
    adjacent_path(target, ".update-new")
}

fn adjacent_path(target: &Path, suffix: &str) -> Result<PathBuf> {
    let name = target
        .file_name()
        .context("managed binary path has no file name")?
        .to_string_lossy();
    Ok(target.with_file_name(format!("{name}{suffix}")))
}

fn same_path(a: &Path, b: &Path) -> bool {
    let a = fs::canonicalize(a).unwrap_or_else(|_| a.to_path_buf());
    let b = fs::canonicalize(b).unwrap_or_else(|_| b.to_path_buf());
    if cfg!(windows) {
        a.to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy())
    } else {
        a == b
    }
}

fn cleanup_alternate_dev_binary(target: &Path) {
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let cargo_enox =
        home.join(".cargo")
            .join("bin")
            .join(if cfg!(windows) { "enox.exe" } else { "enox" });
    if !same_path(&cargo_enox, target) {
        let _ = fs::remove_file(cargo_enox);
    }
    let legacy = if cfg!(windows) { "enoxd.exe" } else { "enoxd" };
    let _ = fs::remove_file(home.join(".cargo").join("bin").join(legacy));
}

fn command_output(path: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new(path).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn command_succeeds(path: &Path, args: &[&str]) -> bool {
    Command::new(path)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(windows)]
fn spawn_windows_apply(
    source: PathBuf,
    target: PathBuf,
    dev_source: PathBuf,
    service: bool,
) -> Result<()> {
    use std::os::windows::process::CommandExt;

    let log_dir = config::enoxian_dir()?.join("logs");
    fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join("update.log");
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    writeln!(log, "\n=== development update handoff ===")?;
    let stderr = log.try_clone()?;

    let mut command = windows_apply_command(&source, &target, &dev_source, service);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr))
        // Detach from the invoking console/job so self-replacement can outlive
        // the old CLI. CREATE_BREAKAWAY_FROM_JOB | CREATE_NEW_PROCESS_GROUP |
        // CREATE_NO_WINDOW.
        .creation_flags(0x0900_0200);
    command.spawn().with_context(|| {
        format!(
            "failed to launch the update handoff; inspect {}",
            log_path.display()
        )
    })?;
    Ok(())
}

#[cfg(windows)]
fn windows_apply_command(
    source: &Path,
    target: &Path,
    dev_source: &Path,
    service: bool,
) -> Command {
    let mut command = Command::new(source);
    command
        .arg("update-apply")
        .arg("--source")
        .arg(source)
        .arg("--target")
        .arg(target)
        .arg("--dev-source")
        .arg(dev_source);
    if service {
        command.arg("--service");
    }
    command
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
        config::save_global(&cfg)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjacent_update_paths_stay_next_to_target() {
        let target = Path::new("/opt/enox/bin/enox");
        assert_eq!(
            backup_path(target).unwrap(),
            Path::new("/opt/enox/bin/enox.update-backup")
        );
        assert_eq!(
            staged_path(target).unwrap(),
            Path::new("/opt/enox/bin/enox.update-new")
        );
    }

    #[test]
    fn release_binary_uses_platform_executable_name() {
        let binary = release_binary(Path::new("/src/enoxian"));
        assert_eq!(
            binary.file_name().unwrap(),
            if cfg!(windows) { "enox.exe" } else { "enox" }
        );
    }

    #[test]
    fn staged_binary_replaces_existing_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir
            .path()
            .join(if cfg!(windows) { "enox.exe" } else { "enox" });
        let staged = staged_path(&target).unwrap();
        fs::write(&target, b"old").unwrap();
        fs::write(&staged, b"new").unwrap();
        replace_once(&staged, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert!(!staged.exists());
    }
}
