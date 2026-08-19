use crate::cli::ServiceAction;
use anyhow::{bail, Context, Result};
use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::process::Stdio;

#[cfg(target_os = "linux")]
const SERVICE_NAME: &str = "enoxian";

pub async fn run(action: ServiceAction, client: &reqwest::Client, daemon_root: &str) -> Result<()> {
    match action {
        ServiceAction::Install {
            port,
            bind_lan,
            bind,
            force,
        } => install(port, bind_lan, bind, force),
        ServiceAction::Status => status(),
        ServiceAction::Start => start(),
        ServiceAction::Stop => {
            let _ = crate::commands::stop::run(client, daemon_root).await;
            stop()
        }
        ServiceAction::Restart => {
            let _ = crate::commands::stop::run(client, daemon_root).await;
            stop()?;
            start()
        }
        ServiceAction::Logs => logs(),
        ServiceAction::Uninstall => {
            let _ = crate::commands::stop::run(client, daemon_root).await;
            uninstall()
        }
    }
}

pub fn is_installed() -> bool {
    service_definition().is_file()
}

pub fn start() -> Result<()> {
    if !is_installed() {
        bail!("managed service is not installed — run `enox service install`");
    }

    #[cfg(target_os = "linux")]
    run_checked(
        systemctl().args(["start", SERVICE_NAME]),
        "start systemd user service",
    )?;

    #[cfg(target_os = "macos")]
    {
        let domain = launchd_domain()?;
        let label = format!("{domain}/com.enoxian.service");
        if !Command::new("launchctl")
            .args(["print", &label])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            let definition = service_definition();
            run_checked(
                Command::new("launchctl").args([
                    "bootstrap",
                    &domain,
                    &definition.to_string_lossy(),
                ]),
                "load LaunchAgent",
            )?;
        }
        run_checked(
            Command::new("launchctl").args(["kickstart", "-k", &label]),
            "start LaunchAgent",
        )?;
    }

    #[cfg(windows)]
    {
        migrate_windows_task_to_windowless_launcher()?;
        run_checked(
            Command::new("schtasks").args(["/Run", "/TN", "Enoxian"]),
            "start scheduled task",
        )?;
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    bail!("managed services are not supported on this platform");

    println!("✓ Enoxian service started");
    Ok(())
}

fn install(port: u16, bind_lan: bool, bind: Option<IpAddr>, force: bool) -> Result<()> {
    let definition = service_definition();
    #[cfg(windows)]
    let stale_definition = definition.exists() && !windows_task_exists();
    #[cfg(not(windows))]
    let stale_definition = false;

    if definition.exists() && !force && !stale_definition {
        bail!(
            "managed service already exists at {} — use --force to replace it",
            definition.display()
        );
    }
    if definition.exists() && !stale_definition {
        stop()?;
    }
    if let Some(parent) = definition.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let exe = std::env::current_exe().context("failed to locate the enox executable")?;
    let daemon_args = daemon_args(port, bind_lan, bind);

    #[cfg(target_os = "linux")]
    {
        fs::write(&definition, systemd_unit(&exe, &daemon_args))
            .with_context(|| format!("failed to write {}", definition.display()))?;
        run_checked(
            systemctl().arg("daemon-reload"),
            "reload systemd user units",
        )?;
        run_checked(
            systemctl().args(["enable", "--now", SERVICE_NAME]),
            "enable systemd user service",
        )?;
    }

    #[cfg(target_os = "macos")]
    {
        let log_dir = state_dir().join("logs");
        fs::create_dir_all(&log_dir)?;
        fs::write(&definition, launch_agent(&exe, &daemon_args, &log_dir))
            .with_context(|| format!("failed to write {}", definition.display()))?;
        let domain = launchd_domain()?;
        let label = format!("{domain}/com.enoxian.service");
        let _ = Command::new("launchctl")
            .args(["bootout", &label])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        run_checked(
            Command::new("launchctl").args(["bootstrap", &domain, &definition.to_string_lossy()]),
            "install LaunchAgent",
        )?;
    }

    #[cfg(windows)]
    {
        let log_dir = state_dir().join("logs");
        fs::create_dir_all(&log_dir)?;
        let wrapper = definition
            .parent()
            .expect("service definition has a parent")
            .join("run.cmd");
        let launcher = definition
            .parent()
            .expect("service definition has a parent")
            .join("run.vbs");
        let command_line = std::iter::once(exe.to_string_lossy().to_string())
            .chain(daemon_args.iter().cloned())
            .map(|part| format!("\"{}\"", part.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" ");
        fs::write(
            &wrapper,
            format!(
                "@echo off\r\n{command_line} >> \"{}\" 2>&1\r\n",
                log_dir.join("service.log").display()
            ),
        )?;
        write_windows_utf16(&launcher, &windows_launcher(&wrapper))?;
        let user_id = windows_user_id()?;
        write_windows_task(&definition, &windows_task(&launcher, &user_id))?;
        if let Err(error) = run_checked(
            Command::new("schtasks").args([
                "/Create",
                "/TN",
                "Enoxian",
                "/XML",
                &definition.to_string_lossy(),
                "/F",
            ]),
            "install scheduled task",
        ) {
            let _ = remove_if_exists(&definition);
            let _ = remove_if_exists(&wrapper);
            let _ = remove_if_exists(&launcher);
            return Err(error);
        }
        start()?;
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    bail!("managed services are not supported on this platform");

    println!("✓ Enoxian will start automatically when you sign in");
    println!("  definition: {}", definition.display());
    println!("  disable: enox service uninstall");
    remember_managed_executable(&exe);
    Ok(())
}

fn remember_managed_executable(exe: &Path) {
    let mut cfg = crate::config::load_global();
    cfg.managed_executable = Some(exe.to_string_lossy().into_owned());
    let _ = crate::config::save_global(&cfg);
}

pub fn installed_executable() -> Option<PathBuf> {
    if !is_installed() {
        return None;
    }

    #[cfg(windows)]
    {
        let wrapper = service_definition().parent()?.join("run.cmd");
        let contents = fs::read_to_string(wrapper).ok()?;
        return windows_executable_from_wrapper(&contents);
    }

    #[cfg(target_os = "linux")]
    {
        let contents = fs::read_to_string(service_definition()).ok()?;
        let command = contents
            .lines()
            .find_map(|line| line.strip_prefix("ExecStart=\""))?;
        let end = command.find('"')?;
        return Some(PathBuf::from(&command[..end]));
    }

    #[cfg(target_os = "macos")]
    {
        let contents = fs::read_to_string(service_definition()).ok()?;
        let arguments = contents.split("<key>ProgramArguments</key>").nth(1)?;
        let first = arguments.split("<string>").nth(1)?;
        let end = first.find("</string>")?;
        return Some(PathBuf::from(xml_unescape(&first[..end])));
    }

    #[allow(unreachable_code)]
    None
}

#[cfg(windows)]
fn windows_executable_from_wrapper(contents: &str) -> Option<PathBuf> {
    let command = contents
        .lines()
        .find(|line| line.trim_start().starts_with('"'))?;
    let rest = command.trim_start().strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(PathBuf::from(&rest[..end]))
}

#[cfg(target_os = "macos")]
fn xml_unescape(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&gt;", ">")
        .replace("&lt;", "<")
        .replace("&amp;", "&")
}

fn stop() -> Result<()> {
    if !is_installed() {
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    run_checked(
        systemctl().args(["stop", SERVICE_NAME]),
        "stop systemd user service",
    )?;

    #[cfg(target_os = "macos")]
    {
        let label = format!("{}/com.enoxian.service", launchd_domain()?);
        let _ = Command::new("launchctl").args(["bootout", &label]).status();
    }

    #[cfg(windows)]
    {
        let _ = Command::new("schtasks")
            .args(["/End", "/TN", "Enoxian"])
            .status();
        stop_windows_daemons();
    }

    println!("✓ Enoxian service stopped");
    Ok(())
}

fn uninstall() -> Result<()> {
    stop()?;
    let definition = service_definition();

    #[cfg(target_os = "linux")]
    {
        let _ = systemctl().args(["disable", SERVICE_NAME]).status();
        remove_if_exists(&definition)?;
        run_checked(
            systemctl().arg("daemon-reload"),
            "reload systemd user units",
        )?;
    }

    #[cfg(target_os = "macos")]
    remove_if_exists(&definition)?;

    #[cfg(windows)]
    {
        let _ = Command::new("schtasks")
            .args(["/Delete", "/TN", "Enoxian", "/F"])
            .status();
        if let Some(parent) = definition.parent() {
            remove_if_exists(&parent.join("run.cmd"))?;
            remove_if_exists(&parent.join("run.vbs"))?;
        }
        remove_if_exists(&definition)?;
    }

    println!("✓ Enoxian login service removed");
    Ok(())
}

fn status() -> Result<()> {
    let definition = service_definition();
    println!(
        "service: {}",
        if definition.is_file() {
            "installed"
        } else {
            "not installed"
        }
    );
    println!("definition: {}", definition.display());
    if !definition.is_file() {
        println!("next: enox service install");
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        let enabled = command_success(systemctl().args(["is-enabled", SERVICE_NAME]));
        let active = command_success(systemctl().args(["is-active", SERVICE_NAME]));
        println!("enabled: {enabled}");
        println!("running: {active}");
    }

    #[cfg(target_os = "macos")]
    {
        let label = format!("{}/com.enoxian.service", launchd_domain()?);
        println!(
            "running: {}",
            command_success(Command::new("launchctl").args(["print", &label]))
        );
    }

    #[cfg(windows)]
    {
        let status = Command::new("schtasks")
            .args(["/Query", "/TN", "Enoxian", "/FO", "LIST", "/V"])
            .status()
            .context("failed to query scheduled task")?;
        println!("registered: {}", status.success());
    }

    Ok(())
}

fn logs() -> Result<()> {
    if !is_installed() {
        bail!("managed service is not installed");
    }

    #[cfg(target_os = "linux")]
    let status = Command::new("journalctl")
        .args(["--user", "-u", "enoxian.service", "-f"])
        .status();

    #[cfg(target_os = "macos")]
    let status = {
        let log_path = state_dir().join("logs/service.log");
        Command::new("tail")
            .args(["-f", &log_path.to_string_lossy()])
            .status()
    };

    #[cfg(windows)]
    let status = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "Get-Content -Wait -Tail 100 -LiteralPath '{}'",
                state_dir()
                    .join("logs/service.log")
                    .to_string_lossy()
                    .replace('\'', "''")
            ),
        ])
        .status();

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    bail!("managed services are not supported on this platform");

    status.context("failed to open service logs")?;
    Ok(())
}

fn daemon_args(port: u16, bind_lan: bool, bind: Option<IpAddr>) -> Vec<String> {
    let mut args = vec![
        "daemon".to_string(),
        "run".to_string(),
        "--port".to_string(),
        port.to_string(),
    ];
    if bind_lan {
        args.push("--bind-lan".to_string());
    }
    if let Some(ip) = bind {
        args.push("--bind".to_string());
        args.push(ip.to_string());
    }
    args
}

fn state_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".enoxian")
}

fn service_definition() -> PathBuf {
    #[cfg(target_os = "linux")]
    return dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/systemd/user/enoxian.service");

    #[cfg(target_os = "macos")]
    return dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library/LaunchAgents/com.enoxian.service.plist");

    #[cfg(windows)]
    return state_dir().join("service/managed-task.txt");

    #[allow(unreachable_code)]
    state_dir().join("service/unsupported")
}

#[cfg(target_os = "linux")]
fn systemctl() -> Command {
    let mut command = Command::new("systemctl");
    command.arg("--user");
    command
}

#[cfg(target_os = "linux")]
fn systemd_unit(exe: &Path, args: &[String]) -> String {
    let invocation = std::iter::once(unit_quote(&exe.to_string_lossy()))
        .chain(args.iter().map(|arg| unit_quote(arg)))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "[Unit]\nDescription=Enoxian collaboration service\nAfter=network-online.target\nWants=network-online.target\nStartLimitIntervalSec=60\nStartLimitBurst=5\n\n[Service]\nType=simple\nExecStart={invocation}\nRestart=on-failure\nRestartSec=3\n\n[Install]\nWantedBy=default.target\n"
    )
}

#[cfg(target_os = "linux")]
fn unit_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(target_os = "macos")]
fn launch_agent(exe: &Path, args: &[String], log_dir: &Path) -> String {
    let mut program_args = format!(
        "    <string>{}</string>\n",
        xml_escape(&exe.to_string_lossy())
    );
    for arg in args {
        program_args.push_str(&format!("    <string>{}</string>\n", xml_escape(arg)));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key><string>com.enoxian.service</string>\n  <key>ProgramArguments</key>\n  <array>\n{program_args}  </array>\n  <key>RunAtLoad</key><true/>\n  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>\n  <key>ThrottleInterval</key><integer>3</integer>\n  <key>StandardOutPath</key><string>{}</string>\n  <key>StandardErrorPath</key><string>{}</string>\n</dict>\n</plist>\n",
        xml_escape(&log_dir.join("service.log").to_string_lossy()),
        xml_escape(&log_dir.join("service.err.log").to_string_lossy())
    )
}

#[cfg(windows)]
fn windows_launcher(wrapper: &Path) -> String {
    let wrapper = wrapper.to_string_lossy().replace('"', "\"\"");
    format!(
        "Option Explicit\r\nDim shell\r\nSet shell = CreateObject(\"WScript.Shell\")\r\nWScript.Quit shell.Run(\"\"\"{wrapper}\"\"\", 0, True)\r\n"
    )
}

#[cfg(windows)]
fn windows_task(launcher: &Path, user_id: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\n<Task version=\"1.4\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\n  <RegistrationInfo><Description>Enoxian collaboration service</Description></RegistrationInfo>\n  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{}</UserId></LogonTrigger></Triggers>\n  <Principals><Principal id=\"Author\"><UserId>{}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>\n  <Settings>\n    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>\n    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>\n    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>\n    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>\n    <RestartOnFailure><Interval>PT1M</Interval><Count>5</Count></RestartOnFailure>\n    <Enabled>true</Enabled>\n  </Settings>\n  <Actions Context=\"Author\"><Exec><Command>wscript.exe</Command><Arguments>//B //NoLogo &quot;{}&quot;</Arguments></Exec></Actions>\n</Task>\n",
        xml_escape(user_id),
        xml_escape(user_id),
        xml_escape(&launcher.to_string_lossy())
    )
}

#[cfg(windows)]
fn windows_user_id() -> Result<String> {
    let username = std::env::var("USERNAME").context("USERNAME is not set")?;
    Ok(std::env::var("USERDOMAIN")
        .map(|domain| format!("{domain}\\{username}"))
        .unwrap_or(username))
}

#[cfg(windows)]
fn migrate_windows_task_to_windowless_launcher() -> Result<()> {
    let definition = service_definition();
    let existing = read_windows_task(&definition)?;
    if existing.contains("<Command>wscript.exe</Command>") {
        return Ok(());
    }

    let wrapper = definition
        .parent()
        .expect("service definition has a parent")
        .join("run.cmd");
    if !wrapper.is_file() {
        bail!(
            "legacy Windows service wrapper is missing at {} — run enox service install --force",
            wrapper.display()
        );
    }
    let launcher = definition
        .parent()
        .expect("service definition has a parent")
        .join("run.vbs");
    write_windows_utf16(&launcher, &windows_launcher(&wrapper))?;

    let _ = Command::new("schtasks")
        .args(["/End", "/TN", "Enoxian"])
        .status();
    stop_windows_daemons();
    write_windows_task(&definition, &windows_task(&launcher, &windows_user_id()?))?;
    run_checked(
        Command::new("schtasks").args([
            "/Create",
            "/TN",
            "Enoxian",
            "/XML",
            &definition.to_string_lossy(),
            "/F",
        ]),
        "migrate scheduled task to windowless background startup",
    )?;
    println!("✓ Migrated Enoxian service to windowless background startup");
    Ok(())
}

#[cfg(windows)]
fn stop_windows_daemons() {
    const SCRIPT: &str = "Get-CimInstance Win32_Process -Filter \"Name = 'enox.exe'\" | Where-Object { $_.CommandLine -match '(?i)(^|\\s)\\\"?daemon\\\"?\\s+\\\"?run\\\"?(\\s|$)' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }";
    let _ = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(windows)]
fn write_windows_task(path: &Path, xml: &str) -> Result<()> {
    write_windows_utf16(path, xml)
}

#[cfg(windows)]
fn write_windows_utf16(path: &Path, contents: &str) -> Result<()> {
    let mut bytes = Vec::with_capacity(2 + contents.len() * 2);
    bytes.extend_from_slice(&[0xff, 0xfe]);
    for unit in contents.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    fs::write(path, bytes).with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(windows)]
fn read_windows_task(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    if bytes.starts_with(&[0xff, 0xfe]) {
        let units = bytes[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        return String::from_utf16(&units)
            .with_context(|| format!("invalid UTF-16 in {}", path.display()));
    }
    String::from_utf8(bytes).with_context(|| format!("invalid UTF-8 in {}", path.display()))
}

#[cfg(windows)]
fn windows_task_exists() -> bool {
    Command::new("schtasks")
        .args(["/Query", "/TN", "Enoxian"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn launchd_domain() -> Result<String> {
    let output = Command::new("id")
        .arg("-u")
        .output()
        .context("failed to determine the current uid")?;
    if !output.status.success() {
        bail!("`id -u` failed");
    }
    Ok(format!(
        "gui/{}",
        String::from_utf8_lossy(&output.stdout).trim()
    ))
}

#[cfg(any(target_os = "macos", windows))]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn command_success(command: &mut Command) -> bool {
    command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn run_checked(command: &mut Command, action: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("failed to {action}"))?;
    if !status.success() {
        bail!("failed to {action} (exit status {status})");
    }
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("failed to remove {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_arguments_keep_loopback_as_the_default() {
        assert_eq!(
            daemon_args(36521, false, None),
            ["daemon", "run", "--port", "36521"]
        );
    }

    #[test]
    fn daemon_arguments_preserve_explicit_network_options() {
        assert_eq!(
            daemon_args(4000, true, Some("127.0.0.2".parse().unwrap())),
            [
                "daemon",
                "run",
                "--port",
                "4000",
                "--bind-lan",
                "--bind",
                "127.0.0.2"
            ]
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_definition_is_login_scoped_and_recovers_from_failure() {
        let task = windows_task(
            Path::new(r"C:\Users\A & B\.enoxian\service\run.vbs"),
            r"DESKTOP\A&B",
        );
        assert!(task.contains("<LogonTrigger>"));
        assert!(task.contains("<RestartOnFailure>"));
        assert!(task.contains("<RunLevel>LeastPrivilege</RunLevel>"));
        assert!(task.contains(r"C:\Users\A &amp; B"));
        assert!(task.contains(r"DESKTOP\A&amp;B"));
        assert!(task.contains("<Command>wscript.exe</Command>"));
        assert!(task.contains("//B //NoLogo"));
        assert!(!task.contains("<Command>cmd.exe</Command>"));
        assert!(!task.contains("<Command>powershell.exe</Command>"));
        assert!(task.starts_with(r#"<?xml version="1.0" encoding="UTF-16"?>"#));
    }

    #[cfg(windows)]
    #[test]
    fn windows_launcher_runs_the_wrapper_without_a_window_and_waits() {
        let launcher = windows_launcher(Path::new(r#"C:\Users\A "quoted"\run.cmd"#));
        assert!(launcher.contains(r#"shell.Run("""C:\Users\A ""quoted""\run.cmd""", 0, True)"#));
    }

    #[cfg(windows)]
    #[test]
    fn windows_definition_is_written_as_utf16le_with_bom() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("task.xml");
        let xml = windows_task(Path::new(r"C:\enox\run.vbs"), r"DESKTOP\user");
        write_windows_task(&path, &xml).unwrap();

        let bytes = fs::read(&path).unwrap();
        assert_eq!(&bytes[..2], &[0xff, 0xfe]);
        assert_eq!(read_windows_task(&path).unwrap(), xml);
    }

    #[cfg(windows)]
    #[test]
    fn windows_wrapper_exposes_the_managed_executable() {
        let wrapper = "@echo off\r\n\"C:\\Program Files\\enoxian\\enox.exe\" \"daemon\" \"run\" >> log 2>&1\r\n";
        assert_eq!(
            windows_executable_from_wrapper(wrapper).unwrap(),
            PathBuf::from(r"C:\Program Files\enoxian\enox.exe")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn systemd_definition_restarts_only_after_failure() {
        let unit = systemd_unit(
            Path::new("/home/a user/bin/enox"),
            &daemon_args(36521, false, None),
        );
        assert!(unit.contains("ExecStart=\"/home/a user/bin/enox\" \"daemon\" \"run\""));
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("WantedBy=default.target"));
    }
}
