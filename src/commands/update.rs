use crate::{cli::UpdateApplyArgs, config};
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
#[cfg(windows)]
use std::fs::OpenOptions;
#[cfg(windows)]
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const REPO: &str = "suzent/enoxian";
const CHANNEL_DEV: &str = "dev";
const CHANNEL_STABLE: &str = "stable";
const CHILD_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(20);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

pub async fn run(
    dev: bool,
    src: Option<PathBuf>,
    no_pull: bool,
    status: bool,
    check: bool,
    release: Option<String>,
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
        run_stable(release, check).await
    }
}

/// Stable channel: download the release archive published for this platform,
/// verify it against the release SHA256SUMS, then hand it to the same staged
/// install/rollback path the development channel uses.
async fn run_stable(release: Option<String>, check: bool) -> Result<()> {
    let service = crate::commands::service::is_installed();
    let target = managed_target(service)?;
    let installed = installed_version(&target);

    let tag = match release.as_deref() {
        Some(requested) => normalize_tag(requested),
        None => latest_tag().await?,
    };
    let wanted = tag.trim_start_matches('v').to_string();

    println!(
        "installed: {}",
        installed.as_deref().unwrap_or("unavailable")
    );
    println!("available: {wanted}");

    let up_to_date = installed.as_deref() == Some(wanted.as_str());
    if check {
        if up_to_date {
            println!("✓ Enoxian is on the newest stable release");
        } else {
            println!("▶ Run `enox update` to install {tag}");
        }
        return Ok(());
    }
    if up_to_date && release.is_none() {
        println!("✓ Enoxian is already up to date");
        return Ok(());
    }

    let staging = staging_dir()?;
    let source = download_release(&tag, &staging).await?;
    verify_binary(&source).context("downloaded release binary failed its pre-install check")?;
    if let Some(found) = version_of(&source) {
        if found != wanted {
            let _ = fs::remove_dir_all(&staging);
            bail!("downloaded binary reports version '{found}', expected '{wanted}'");
        }
    }

    println!("▶ Stopping Enoxian...");
    stop_current(service)?;

    #[cfg(windows)]
    {
        spawn_windows_apply(source, target, None, service)?;
        println!("▶ Handed off to the verified release binary...");
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
            dev_source: None,
        })
    }
}

fn normalize_tag(requested: &str) -> String {
    let trimmed = requested.trim();
    if trimmed.starts_with('v') {
        trimmed.to_string()
    } else {
        format!("v{trimmed}")
    }
}

async fn latest_tag() -> Result<String> {
    #[derive(serde::Deserialize)]
    struct Release {
        tag_name: String,
    }

    println!("▶ Checking for the newest stable release...");
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let release: Release = http_client()?
        .get(&url)
        .header("accept", "application/vnd.github+json")
        .send()
        .await
        .with_context(|| format!("failed to query {url}"))?
        .error_for_status()
        .with_context(|| format!("failed to query {url}"))?
        .json()
        .await
        .context("release metadata was not valid JSON")?;
    if release.tag_name.trim().is_empty() {
        bail!("the latest release has no tag name");
    }
    Ok(release.tag_name)
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(concat!("enox/", env!("CARGO_PKG_VERSION")))
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .context("failed to build the update HTTP client")
}

/// Release asset published for this platform by `.github/workflows/release.yml`.
fn asset_name() -> Result<&'static str> {
    Ok(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "enoxian-linux-x86_64.tar.gz",
        ("linux", "aarch64") => "enoxian-linux-aarch64.tar.gz",
        ("macos", "aarch64") => "enoxian-macos-aarch64.tar.gz",
        ("macos", "x86_64") => "enoxian-macos-x86_64.tar.gz",
        ("windows", "x86_64") => "enoxian-windows-x86_64.zip",
        (os, arch) => bail!(
            "no stable release is published for {os}/{arch}; build from source with `enox update --dev --src <path>`"
        ),
    })
}

fn staging_dir() -> Result<PathBuf> {
    let dir = config::enoxian_dir()?.join("update");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    Ok(dir)
}

/// Downloads and checksum-verifies the release archive, returning the path of
/// the extracted `enox` executable inside `staging`.
async fn download_release(tag: &str, staging: &Path) -> Result<PathBuf> {
    let asset = asset_name()?;
    let base = format!("https://github.com/{REPO}/releases/download/{tag}");
    let client = http_client()?;

    println!("▶ Downloading {asset} ({tag})...");
    let archive = fetch_bytes(&client, &format!("{base}/{asset}")).await?;
    let sums = fetch_bytes(&client, &format!("{base}/SHA256SUMS")).await?;
    let sums = String::from_utf8(sums).context("SHA256SUMS is not valid UTF-8")?;

    let expected = expected_checksum(&sums, asset)
        .with_context(|| format!("SHA256SUMS has no entry for {asset}"))?;
    let actual = hex::encode(Sha256::digest(&archive));
    if actual != expected {
        bail!("checksum mismatch for {asset}; the download was discarded");
    }
    println!("✓ Checksum verified");

    let archive_path = staging.join(asset);
    fs::write(&archive_path, &archive)
        .with_context(|| format!("failed to write {}", archive_path.display()))?;
    extract(&archive_path, staging)?;
    let _ = fs::remove_file(&archive_path);

    let binary = staging.join(if cfg!(windows) { "enox.exe" } else { "enox" });
    if !binary.is_file() {
        bail!("{asset} does not contain an enox executable");
    }
    make_executable(&binary)?;
    Ok(binary)
}

async fn fetch_bytes(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("failed to download {url}"))?
        .error_for_status()
        .with_context(|| format!("failed to download {url}"))?;
    Ok(response
        .bytes()
        .await
        .with_context(|| format!("failed to read the response body of {url}"))?
        .to_vec())
}

/// SHA256SUMS lines are `<hash>  <name>`; GNU coreutils writes `*<name>` for
/// binary mode, so both spellings are accepted.
fn expected_checksum(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, name) = line.split_once(char::is_whitespace)?;
        let name = name.trim().trim_start_matches('*');
        (name == asset).then(|| hash.trim().to_ascii_lowercase())
    })
}

fn extract(archive: &Path, into: &Path) -> Result<()> {
    let status = if cfg!(windows) {
        Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command"])
            .arg(format!(
                "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
                archive.display(),
                into.display()
            ))
            .status()
    } else {
        Command::new("tar")
            .arg("-C")
            .arg(into)
            .arg("-xzf")
            .arg(archive)
            .status()
    }
    .with_context(|| format!("failed to extract {}", archive.display()))?;
    if !status.success() {
        bail!("failed to extract {}", archive.display());
    }
    Ok(())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .with_context(|| format!("failed to mark {} executable", path.display()))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// `enox --version` prints `enox <semver>`; the bare version is what the
/// release tag carries.
fn version_of(path: &Path) -> Option<String> {
    let output = command_output(path, &["--version"])?;
    output
        .split_whitespace()
        .next_back()
        .map(|value| value.to_string())
}

fn installed_version(target: &Path) -> Option<String> {
    version_of(target)
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
        spawn_windows_apply(source, target, Some(src), service)?;
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
            dev_source: Some(src),
        })
    }
}

pub fn apply(args: UpdateApplyArgs) -> Result<()> {
    #[cfg(windows)]
    thread::sleep(Duration::from_millis(750));

    let dev = args.dev_source.is_some();
    let label = if dev { "development" } else { "release" };
    let backup = backup_path(&args.target)?;
    let staged = staged_path(&args.target)?;
    let had_target = args.target.is_file();

    if same_path(&args.source, &args.target) {
        println!("▶ The {label} binary is already at the managed path.");
    } else {
        println!(
            "▶ Installing {label} binary to {}...",
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
        eprintln!("✗ {label} update failed: {error:#}");
        if !same_path(&args.source, &args.target) {
            rollback(&args.target, &backup, had_target, args.service)?;
        }
        bail!("{label} update rolled back; the previous installation was restored");
    }

    let mut cfg = config::load_global();
    if let Some(dev_source) = &args.dev_source {
        cfg.dev_src = Some(dev_source.to_string_lossy().into_owned());
        cfg.update_channel = Some(CHANNEL_DEV.to_string());
    } else {
        cfg.update_channel = Some(CHANNEL_STABLE.to_string());
    }
    cfg.managed_executable = Some(args.target.to_string_lossy().into_owned());
    config::save_global(&cfg)?;

    let _ = fs::remove_file(&backup);
    let _ = fs::remove_file(&staged);
    if dev {
        cleanup_alternate_dev_binary(&args.target);
    } else {
        discard_staging_dir(&args.source);
    }
    println!("✓ {label} update installed and healthy");
    println!("  binary: {}", args.target.display());
    if let Some(dev_source) = &args.dev_source {
        println!("  source: {}", dev_source.display());
    } else if let Some(version) = version_of(&args.target) {
        println!("  version: {version}");
    }
    Ok(())
}

/// Removes `~/.enoxian/update` once its staged binary has been installed. The
/// path check keeps a `--source` outside the staging area untouched.
fn discard_staging_dir(source: &Path) {
    let Ok(expected) = config::enoxian_dir().map(|dir| dir.join("update")) else {
        return;
    };
    if source.parent() == Some(expected.as_path()) {
        let _ = fs::remove_dir_all(&expected);
    }
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
    if service {
        return crate::commands::service::stop_managed();
    }

    let exe = std::env::current_exe().context("failed to locate the current enox executable")?;
    let mut command = Command::new(exe);
    command.arg("stop");
    let status = command_status_with_timeout(&mut command, CHILD_COMMAND_TIMEOUT)
        .context("failed to stop Enoxian")?;
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
    let deadline = Instant::now() + HEALTH_TIMEOUT;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let attempt_timeout = remaining.min(Duration::from_secs(1));
        let mut command = Command::new(target);
        command
            .args(["circles", "--json"])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let healthy = command_status_with_timeout(&mut command, attempt_timeout)
            .map(|status| status.success())
            .unwrap_or(false);
        if healthy {
            return Ok(());
        }
        thread::sleep(
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(500)),
        );
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
    if service {
        let _ = crate::commands::service::stop_managed();
        return;
    }
    let mut command = Command::new(target);
    command.arg("stop");
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let _ = command_status_with_timeout(&mut command, CHILD_COMMAND_TIMEOUT);
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
    let mut command = Command::new(path);
    command
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command_status_with_timeout(&mut command, CHILD_COMMAND_TIMEOUT)
        .map(|status| status.success())
        .unwrap_or(false)
}

fn command_status_with_timeout(
    command: &mut Command,
    timeout: Duration,
) -> Result<std::process::ExitStatus> {
    let mut child = command.spawn().context("failed to start child process")?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child
            .try_wait()
            .context("failed to wait for child process")?
        {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("child process exceeded {} seconds", timeout.as_secs_f64());
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(windows)]
fn spawn_windows_apply(
    source: PathBuf,
    target: PathBuf,
    dev_source: Option<PathBuf>,
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
    writeln!(log, "\n=== update handoff ===")?;
    let stderr = log.try_clone()?;

    let mut command = windows_apply_command(&source, &target, dev_source.as_deref(), service);
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
    dev_source: Option<&Path>,
    service: bool,
) -> Command {
    let mut command = Command::new(source);
    command
        .arg("update-apply")
        .arg("--source")
        .arg(source)
        .arg("--target")
        .arg(target);
    if let Some(dev_source) = dev_source {
        command.arg("--dev-source").arg(dev_source);
    }
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

    /// Network smoke test for the stable download leg: resolves the newest
    /// release, downloads the asset for this platform, verifies it against
    /// SHA256SUMS, extracts it, and runs the result. Opt in with
    /// `cargo test -- --ignored stable_release_downloads`.
    #[tokio::test]
    #[ignore = "requires network access to github.com"]
    async fn stable_release_downloads_and_verifies() {
        let tag = latest_tag().await.expect("resolve the latest release tag");
        let dir = tempfile::tempdir().unwrap();
        let binary = download_release(&tag, dir.path())
            .await
            .expect("download and verify the release asset");

        assert!(binary.is_file(), "no executable was extracted");
        verify_binary(&binary).expect("downloaded binary failed --version");
        assert_eq!(
            version_of(&binary).as_deref(),
            Some(tag.trim_start_matches('v')),
            "downloaded binary does not report the requested version"
        );
    }

    /// `ENOXIAN_HOME` is process-wide, so the tests that repoint it run under
    /// one lock rather than in parallel.
    static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn staged_source_is_discarded_but_an_external_source_is_kept() {
        let _guard = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let previous = std::env::var("ENOXIAN_HOME").ok();
        std::env::set_var("ENOXIAN_HOME", home.path());

        // A source inside ~/.enoxian/update is ours to clean up.
        let staging = staging_dir().unwrap();
        let staged = staging.join("enox");
        fs::write(&staged, b"binary").unwrap();
        discard_staging_dir(&staged);
        assert!(!staging.exists(), "staging directory should be removed");

        // A source anywhere else (a dev checkout's target/release) is not.
        let external = tempfile::tempdir().unwrap();
        let outside = external.path().join("enox");
        fs::write(&outside, b"binary").unwrap();
        discard_staging_dir(&outside);
        assert!(outside.is_file(), "an external source must not be removed");

        match previous {
            Some(value) => std::env::set_var("ENOXIAN_HOME", value),
            None => std::env::remove_var("ENOXIAN_HOME"),
        }
    }

    #[test]
    fn update_apply_accepts_both_channels() {
        use clap::Parser;

        #[derive(Parser)]
        struct Harness {
            #[command(flatten)]
            args: UpdateApplyArgs,
        }

        // Stable: no --dev-source, so apply() records the stable channel.
        let stable = Harness::parse_from([
            "update-apply",
            "--source",
            "/staging/enox",
            "--target",
            "/usr/local/bin/enox",
        ]);
        assert!(stable.args.dev_source.is_none());
        assert!(!stable.args.service);

        // Development: --dev-source carries the checkout that was built.
        let dev = Harness::parse_from([
            "update-apply",
            "--source",
            "/src/target/release/enox",
            "--target",
            "/usr/local/bin/enox",
            "--dev-source",
            "/src",
            "--service",
        ]);
        assert_eq!(dev.args.dev_source.as_deref(), Some(Path::new("/src")));
        assert!(dev.args.service);
    }

    #[test]
    fn requested_release_gains_a_leading_v() {
        assert_eq!(normalize_tag("0.8.0"), "v0.8.0");
        assert_eq!(normalize_tag(" v0.8.0 "), "v0.8.0");
    }

    #[test]
    fn checksum_lookup_accepts_both_sha256sums_spellings() {
        let sums = concat!(
            "aaaa  enoxian-linux-x86_64.tar.gz\n",
            "BBBB *enoxian-macos-aarch64.tar.gz\n",
        );
        assert_eq!(
            expected_checksum(sums, "enoxian-linux-x86_64.tar.gz").as_deref(),
            Some("aaaa")
        );
        assert_eq!(
            expected_checksum(sums, "enoxian-macos-aarch64.tar.gz").as_deref(),
            Some("bbbb")
        );
        assert!(expected_checksum(sums, "enoxian-windows-x86_64.zip").is_none());
    }

    #[test]
    fn asset_name_matches_the_published_release_matrix() {
        // Unsupported platforms are told to build from source instead.
        if let Ok(asset) = asset_name() {
            assert!(asset.starts_with("enoxian-"));
            assert!(asset.ends_with(".tar.gz") || asset.ends_with(".zip"));
        }
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
