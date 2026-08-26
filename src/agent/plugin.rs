//! Managed agent-adapter plugins.
//!
//! A chat mention must never depend on a live package-manager download. Plugin
//! installation is an explicit control-plane action; after that, the reaction
//! path launches a version-pinned executable from `~/.enoxian/adapters`.
//!
//! Enoxian ships a small built-in catalog and also loads additional TOML
//! manifests from `~/.enoxian/plugins/*.toml`. Third-party manifests use the
//! same installer and health model as built-ins, but cannot override built-in
//! plugin ids.

use super::config::{AgentCommand, AgentConfig, Driver};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime};

/// How a plugin's executable comes to exist on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A pinned npm adapter Enoxian installs and version-locks itself. The
    /// adapter is only transport; it usually bridges to a product CLI.
    #[default]
    Npm,
    /// A product CLI that speaks ACP itself. Enoxian installs nothing, pins
    /// nothing, and needs no Node.js: the user installs the CLI and a mention
    /// launches whatever is on `PATH`. There is no adapter to version, so
    /// `package` and `version` are empty and "install" only means writing the
    /// chat handle into `agents.toml`.
    Native,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Stable plugin id, used as the install-directory name.
    pub id: String,
    /// Chat handle configured after installation, e.g. `codex`.
    pub agent: String,
    /// Exact adapter version. Ranges and `latest` are deliberately rejected.
    /// Empty for [`Kind::Native`], which pins nothing.
    #[serde(default)]
    pub version: String,
    #[serde(default = "default_driver")]
    pub driver: Driver,
    /// npm package containing the adapter executable. Empty for
    /// [`Kind::Native`].
    #[serde(default)]
    pub package: String,
    /// Executable name exposed in `node_modules/.bin`, or — for
    /// [`Kind::Native`] — the CLI name looked up on `PATH`.
    pub binary: String,
    #[serde(default)]
    pub kind: Kind,
    /// Arguments appended after `binary` when launching. A native CLI needs the
    /// subcommand that speaks ACP (`suzent acp`); an npm adapter is launched by
    /// path with no arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// Where to get the CLI, shown when a native plugin's binary is missing.
    #[serde(default)]
    pub install_url: String,
    #[serde(default)]
    pub about: String,
}

impl PluginManifest {
    /// The argv a mention launches: the given executable plus any args.
    fn launch_command(&self, executable: &Path) -> Vec<String> {
        let mut command = vec![executable.to_string_lossy().into_owned()];
        command.extend(self.args.iter().cloned());
        command
    }

    /// The argv this plugin's chat handle should hold in `agents.toml`.
    ///
    /// A native handle follows `PATH` by name, so reinstalling the CLI under a
    /// different prefix cannot strand it. An npm adapter gets its pinned
    /// absolute path, which is what makes mention execution deterministic.
    ///
    /// Both the settings view (to decide whether a handle is current) and the
    /// installer (to write it) go through here — computing it in two places is
    /// how a correctly-configured handle starts reporting itself as legacy.
    fn handle_command(&self, executable: &Path) -> Vec<String> {
        if self.kind == Kind::Native {
            self.launch_command(Path::new(&self.binary))
        } else {
            self.launch_command(executable)
        }
    }
}

fn default_driver() -> Driver {
    Driver::Acp
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginState {
    Missing,
    Installing,
    Broken,
    Ready,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginView {
    pub id: String,
    pub agent: String,
    /// Empty for a native plugin, which pins no version.
    pub version: String,
    pub driver: String,
    /// "npm" or "native".
    pub kind: String,
    /// Whether this plugin needs system Node.js at all. False for a native
    /// plugin, so the UI must not demand a runtime it never uses.
    pub requires_node: bool,
    /// Where to get the CLI when a native plugin is missing.
    pub install_url: String,
    pub package: String,
    pub about: String,
    pub source: String,
    pub state: PluginState,
    pub configured: bool,
    pub legacy_configured: bool,
    pub executable: String,
    /// Whether system Node.js 22+ and npm are ready for adapter installation.
    pub node_runtime_installed: bool,
    pub node_runtime_version: Option<String>,
    /// Required underlying product CLI, when the adapter is only a bridge.
    pub runtime_program: Option<String>,
    /// Whether that underlying CLI is currently resolvable on PATH.
    pub runtime_installed: Option<bool>,
    /// Explicit login command shown when authentication is missing.
    pub runtime_login_command: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CatalogEntry {
    pub manifest: PluginManifest,
    pub source: String,
}

const CODEX_VERSION: &str = "1.1.14";
const CLAUDE_VERSION: &str = "0.69.0";
const CLAUDE_PLUGIN_ID: &str = "claude-agent-acp";

fn builtins() -> Vec<CatalogEntry> {
    vec![
        CatalogEntry {
            manifest: PluginManifest {
                id: "codex-acp".into(),
                agent: "codex".into(),
                version: CODEX_VERSION.into(),
                driver: Driver::Acp,
                package: "@agentclientprotocol/codex-acp".into(),
                binary: "codex-acp".into(),
                kind: Kind::Npm,
                args: Vec::new(),
                install_url: String::new(),
                about: "OpenAI Codex CLI through a pinned ACP transport bridge.".into(),
            },
            source: "builtin".into(),
        },
        CatalogEntry {
            manifest: PluginManifest {
                id: CLAUDE_PLUGIN_ID.into(),
                agent: "claude".into(),
                version: CLAUDE_VERSION.into(),
                driver: Driver::Acp,
                package: "@agentclientprotocol/claude-agent-acp".into(),
                binary: "claude-agent-acp".into(),
                kind: Kind::Npm,
                args: Vec::new(),
                install_url: String::new(),
                about: "Claude Code CLI through a pinned ACP transport bridge.".into(),
            },
            source: "builtin".into(),
        },
        CatalogEntry {
            manifest: PluginManifest {
                id: "suzent".into(),
                agent: "suzent".into(),
                // Nothing to pin: the CLI is the agent.
                version: String::new(),
                driver: Driver::Acp,
                package: String::new(),
                binary: "suzent".into(),
                kind: Kind::Native,
                args: vec!["acp".into()],
                install_url: "https://github.com/cyzus/suzent".into(),
                about: "Your local Suzent, speaking ACP itself — no adapter, no Node.js. \
                        Needs its backend running (`suzent serve`)."
                    .into(),
            },
            source: "builtin".into(),
        },
    ]
}

pub fn plugins_dir() -> Result<PathBuf> {
    Ok(crate::config::enoxian_dir()?.join("plugins"))
}

pub fn adapters_dir() -> Result<PathBuf> {
    Ok(crate::config::enoxian_dir()?.join("adapters"))
}

fn system_node_status() -> (bool, Option<String>) {
    let Some(node) = super::probe::resolve("node") else {
        return (false, None);
    };
    let npm_installed = super::probe::resolve("npm").is_some();
    let output = std::process::Command::new(node).arg("--version").output();
    let Ok(output) = output else {
        return (false, None);
    };
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let compatible =
        output.status.success() && npm_installed && node_major(&version).unwrap_or(0) >= 22;
    (compatible, (!version.is_empty()).then_some(version))
}

/// Built-ins followed by valid third-party manifests. Invalid manifests are
/// ignored with a warning so one plugin cannot break the settings page.
pub fn catalog() -> Vec<CatalogEntry> {
    let mut entries: BTreeMap<String, CatalogEntry> = builtins()
        .into_iter()
        .map(|entry| (entry.manifest.id.clone(), entry))
        .collect();

    let Ok(dir) = plugins_dir() else {
        return entries.into_values().collect();
    };
    let Ok(read) = std::fs::read_dir(&dir) else {
        return entries.into_values().collect();
    };
    for file in read.flatten() {
        let path = file.path();
        if path.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let parsed = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))
            .and_then(|text| {
                toml::from_str::<PluginManifest>(&text).context("invalid plugin manifest")
            })
            .and_then(|manifest| {
                validate_manifest(&manifest)?;
                Ok(manifest)
            });
        match parsed {
            Ok(manifest) if !entries.contains_key(&manifest.id) => {
                entries.insert(
                    manifest.id.clone(),
                    CatalogEntry {
                        manifest,
                        source: path.to_string_lossy().into_owned(),
                    },
                );
            }
            Ok(manifest) => tracing::warn!(
                "[plugin] duplicate id '{}' in {}; built-in/first entry wins",
                manifest.id,
                path.display()
            ),
            Err(e) => tracing::warn!("[plugin] skipping {}: {e:#}", path.display()),
        }
    }
    entries.into_values().collect()
}

pub fn find(id: &str) -> Option<CatalogEntry> {
    let canonical = match id {
        "claude" | "claude-code-acp" => CLAUDE_PLUGIN_ID,
        other => other,
    };
    catalog()
        .into_iter()
        .find(|entry| entry.manifest.id == canonical)
}

fn validate_manifest(m: &PluginManifest) -> Result<()> {
    for (label, value) in [("id", &m.id), ("agent", &m.agent), ("binary", &m.binary)] {
        if value.is_empty()
            || !value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            bail!("{label} must contain only letters, digits, '-', '_' or '.'");
        }
    }
    // A native plugin has no package to fetch and no version to pin: the CLI on
    // PATH is whatever the user installed, so requiring either would be a
    // fiction. Everything else about it is validated the same way.
    if m.kind == Kind::Native {
        if !m.package.trim().is_empty() {
            bail!("a native plugin installs nothing, so it must not name a package");
        }
        if !m.version.trim().is_empty() {
            bail!("a native plugin pins nothing, so it must not claim a version");
        }
        return Ok(());
    }
    if m.package.trim().is_empty() || m.package.chars().any(char::is_whitespace) {
        bail!("package must be a non-empty npm package name without whitespace");
    }
    if m.version.is_empty()
        || m.version == "latest"
        || m.version.contains('*')
        || m.version.contains('^')
        || m.version.contains('~')
        || m.version.chars().any(char::is_whitespace)
    {
        bail!("version must be exact (not latest or a range)");
    }
    if !m.args.is_empty() {
        bail!("an npm adapter is launched by path, so it must not carry args");
    }
    Ok(())
}

fn install_root(base: &Path, manifest: &PluginManifest) -> PathBuf {
    base.join(&manifest.id).join(&manifest.version)
}

fn executable_at(base: &Path, manifest: &PluginManifest) -> PathBuf {
    if manifest.kind == Kind::Native {
        // Follow PATH rather than freezing an absolute path: the user owns this
        // CLI, and reinstalling it under a different prefix must not strand the
        // configured handle. Fall back to the bare name when it is absent, so
        // the view still has something to show.
        return super::probe::resolve(&manifest.binary)
            .unwrap_or_else(|| PathBuf::from(&manifest.binary));
    }
    let bin = install_root(base, manifest)
        .join("node_modules")
        .join(".bin");
    #[cfg(windows)]
    {
        bin.join(format!("{}.cmd", manifest.binary))
    }
    #[cfg(not(windows))]
    {
        bin.join(&manifest.binary)
    }
}

pub fn executable(manifest: &PluginManifest) -> Result<PathBuf> {
    Ok(executable_at(&adapters_dir()?, manifest))
}

fn state_at(base: &Path, manifest: &PluginManifest) -> PluginState {
    if manifest.kind == Kind::Native {
        // Nothing is ever installing or half-installed: the CLI is either on
        // PATH or it is not.
        return if super::probe::is_installed(&manifest.binary) {
            PluginState::Ready
        } else {
            PluginState::Missing
        };
    }
    let root = install_root(base, manifest);
    if root.join(".installing").exists() {
        return PluginState::Installing;
    }
    if executable_at(base, manifest).is_file() {
        PluginState::Ready
    } else if root.exists() {
        PluginState::Broken
    } else {
        PluginState::Missing
    }
}

pub fn views() -> Vec<PluginView> {
    let cfg = AgentConfig::load();
    let base = adapters_dir().unwrap_or_default();
    let (node_runtime_installed, node_runtime_version) = system_node_status();
    catalog()
        .into_iter()
        .map(|entry| {
            let manifest = entry.manifest;
            let executable = executable_at(&base, &manifest);
            let expected = manifest.handle_command(&executable);
            let configured_cmd = cfg.resolve(&manifest.agent);
            let native = manifest.kind == Kind::Native;
            // A native plugin's "bridged CLI" is its own binary: there is no
            // adapter in front of it, so its presence is the prerequisite.
            let bridge = super::probe::bridged_cli(&manifest.binary);
            PluginView {
                id: manifest.id.clone(),
                agent: manifest.agent.clone(),
                version: manifest.version.clone(),
                driver: format!("{:?}", manifest.driver).to_lowercase(),
                kind: if native {
                    "native".into()
                } else {
                    "npm".into()
                },
                requires_node: !native,
                install_url: manifest.install_url.clone(),
                package: manifest.package.clone(),
                about: manifest.about.clone(),
                source: entry.source,
                state: state_at(&base, &manifest),
                configured: configured_cmd
                    .map(|c| c.command == expected)
                    .unwrap_or(false),
                legacy_configured: configured_cmd
                    .map(|c| c.command != expected)
                    .unwrap_or(false),
                executable: executable.to_string_lossy().into_owned(),
                node_runtime_installed: if native { true } else { node_runtime_installed },
                node_runtime_version: if native {
                    None
                } else {
                    node_runtime_version.clone()
                },
                runtime_program: if native {
                    Some(manifest.binary.clone())
                } else {
                    bridge.map(|bridge| bridge.program.to_string())
                },
                runtime_installed: if native {
                    Some(super::probe::is_installed(&manifest.binary))
                } else {
                    bridge.map(|bridge| super::probe::is_installed(bridge.program))
                },
                runtime_login_command: if native {
                    None
                } else {
                    bridge.map(|bridge| bridge.login_command.to_string())
                },
            }
        })
        .collect()
}

struct InstallGuard(PathBuf);
impl Drop for InstallGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

async fn require_system_npm() -> Result<PathBuf> {
    let node = super::probe::resolve("node").context(
        "agent adapters require system Node.js 22+ with npm; install it from https://nodejs.org and restart Enoxian",
    )?;
    let args = vec!["--version".to_string()];
    let output = super::spawn::command(&node.to_string_lossy(), &args)
        .stdin(Stdio::null())
        .output()
        .await
        .context("failed to check the system Node.js version")?;
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || node_major(&version).unwrap_or(0) < 22 {
        bail!(
            "agent adapters require system Node.js 22+ with npm (found '{}'); update Node.js and restart Enoxian",
            if version.is_empty() { "unknown" } else { &version }
        );
    }
    super::probe::resolve("npm").context(
        "agent adapters require npm, but it was not found on PATH; install npm and restart Enoxian",
    )
}

/// Install a pinned adapter and configure its chat handle to launch the exact
/// managed executable. This is the only networked phase; mention execution is
/// offline and deterministic afterwards.
pub async fn install(id: &str) -> Result<AgentCommand> {
    let entry = find(id).with_context(|| format!("unknown agent plugin '{id}'"))?;
    let manifest = entry.manifest;
    validate_manifest(&manifest)?;
    if manifest.kind == Kind::Native {
        return configure_native(&manifest);
    }
    if let Some(bridge) = super::probe::bridged_cli(&manifest.binary) {
        verify_bridged_runtime(bridge).await?;
    }
    let npm = require_system_npm().await?;
    let base = adapters_dir()?;
    let root = install_root(&base, &manifest);
    std::fs::create_dir_all(&root)?;
    let lock = root.join(".installing");
    // A killed daemon must not leave this plugin permanently unrepairable.
    // A live install is bounded to five minutes by the API, so a ten-minute
    // marker cannot belong to an active supported install.
    if lock.exists() {
        let stale = lock
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .map(|age| age > Duration::from_secs(10 * 60))
            .unwrap_or(false);
        if stale {
            std::fs::remove_file(&lock)
                .with_context(|| format!("removing stale install lock for '{}'", manifest.id))?;
        }
    }
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock)
        .with_context(|| format!("plugin '{}' is already installing", manifest.id))?;
    let _guard = InstallGuard(lock);

    let spec = format!("{}@{}", manifest.package, manifest.version);
    let args = vec![
        "install".into(),
        "--prefix".into(),
        root.to_string_lossy().into_owned(),
        "--no-save".into(),
        "--no-package-lock".into(),
        "--no-audit".into(),
        "--no-fund".into(),
        "--loglevel".into(),
        "error".into(),
        "--".into(),
        spec,
    ];
    let mut command = super::spawn::command(&npm.to_string_lossy(), &args);
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = command
        .output()
        .await
        .context("failed to start the adapter npm runtime")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr
            .lines()
            .rev()
            .take(8)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        bail!("npm install failed ({}): {}", output.status, detail.trim());
    }

    let exe = executable_at(&base, &manifest);
    if !exe.is_file() {
        bail!("plugin installed but '{}' was not created", exe.display());
    }
    configure_handle(&manifest, &exe)
}

/// "Install" a native plugin: there is nothing to download, so this only
/// verifies the CLI is really here and writes the chat handle.
///
/// Failing here keeps the actionable message on the install action rather than
/// letting it surface later as an opaque spawn error on a chat mention.
fn configure_native(manifest: &PluginManifest) -> Result<AgentCommand> {
    let exe = super::probe::resolve(&manifest.binary).with_context(|| {
        format!(
            "the {} CLI is required but was not found on PATH. Install it from {}, then check again",
            manifest.binary, manifest.install_url
        )
    })?;
    configure_handle(manifest, &exe)
}

/// Point the plugin's chat handle at `executable`, preserving any working_dir
/// the operator had set on that handle.
fn configure_handle(manifest: &PluginManifest, executable: &Path) -> Result<AgentCommand> {
    let mut cfg = AgentConfig::load_for_edit()?;
    let working_dir = cfg
        .resolve(&manifest.agent)
        .and_then(|existing| existing.working_dir.clone());
    let command = AgentCommand {
        command: manifest.handle_command(executable),
        driver: manifest.driver,
        working_dir,
    };
    cfg.set_agent(&manifest.agent, command.clone());
    cfg.save()?;
    Ok(command)
}

/// A bridge cannot work without the CLI it bridges to, so refuse to install one
/// whose CLI is absent — or, where the CLI can report it, signed out. Failing
/// here keeps the actionable message on the install action, instead of letting
/// it surface later as an opaque session error on a chat mention.
async fn verify_bridged_runtime(bridge: &super::probe::BridgedCli) -> Result<()> {
    let cli = super::probe::resolve(bridge.program).with_context(|| {
        format!(
            "the {} CLI is required but was not found on PATH. Install it from {}, then run `{}`",
            bridge.program, bridge.install_url, bridge.login_command
        )
    })?;

    let Some(auth_args) = bridge.auth_status_args else {
        return Ok(());
    };
    let auth_args: Vec<String> = auth_args.iter().map(|arg| (*arg).to_string()).collect();
    let auth = super::spawn::command(&cli.to_string_lossy(), &auth_args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .with_context(|| format!("failed to check {} authentication", bridge.program))?;
    if !auth.status.success() {
        bail!(
            "the {} CLI is installed but not authenticated. Run `{}`, then retry the install",
            bridge.program,
            bridge.login_command
        );
    }
    Ok(())
}

fn node_major(version: &str) -> Option<u32> {
    version
        .trim()
        .trim_start_matches('v')
        .split('.')
        .next()?
        .parse()
        .ok()
}

/// Detect legacy runtime package-manager launchers. They may technically be on
/// PATH, but they are not ready for deterministic mention execution.
pub fn command_status(command: &[String]) -> &'static str {
    let Some(program) = command.first() else {
        return "missing";
    };
    let file = Path::new(program)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(program);
    if matches!(
        file.to_ascii_lowercase().as_str(),
        "npx" | "npm" | "pnpm" | "yarn"
    ) {
        return "runtime_download";
    }
    if super::probe::is_installed(program) {
        "ready"
    } else {
        "missing"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_exact_and_valid() {
        for entry in builtins() {
            validate_manifest(&entry.manifest).unwrap();
            assert!(!entry.manifest.version.contains('*'));
        }
    }

    #[test]
    fn managed_executable_is_versioned() {
        let base = Path::new("root");
        let manifest = &builtins()[0].manifest;
        let path = executable_at(base, manifest)
            .to_string_lossy()
            .replace('\\', "/");
        assert!(path.contains("codex-acp/1.1.14/node_modules/.bin/codex-acp"));
    }

    #[test]
    fn claude_aliases_resolve_to_current_bridge() {
        for id in ["claude", "claude-code-acp", "claude-agent-acp"] {
            let entry = find(id).expect("Claude alias should resolve");
            assert_eq!(entry.manifest.id, "claude-agent-acp");
            assert_eq!(
                entry.manifest.package,
                "@agentclientprotocol/claude-agent-acp"
            );
            assert_eq!(entry.manifest.binary, "claude-agent-acp");
        }
    }

    #[test]
    fn every_builtin_reaches_a_product_cli_the_user_installs() {
        // No built-in is a self-contained runtime: each one ends at a CLI the
        // user owns, which is what makes the settings page report a real
        // prerequisite. There are two ways to get there, and every built-in
        // must take exactly one of them.
        for entry in builtins() {
            let manifest = entry.manifest;
            match manifest.kind {
                // An npm adapter is only transport, so it must resolve a bridge.
                Kind::Npm => {
                    let bridge = super::super::probe::bridged_cli(&manifest.binary)
                        .unwrap_or_else(|| panic!("{} should bridge to a CLI", manifest.id));
                    assert!(!bridge.program.is_empty());
                    assert!(!bridge.executable_env.is_empty());
                }
                // A native plugin *is* the CLI, so it must say where to get it
                // and how to launch it — there is no adapter to explain either.
                Kind::Native => {
                    assert!(
                        super::super::probe::bridged_cli(&manifest.binary).is_none(),
                        "{} is the product CLI, so it must not also bridge to one",
                        manifest.id
                    );
                    assert!(
                        !manifest.install_url.is_empty(),
                        "{} must say where to install its CLI from",
                        manifest.id
                    );
                    assert!(
                        !manifest.args.is_empty(),
                        "{} must name the subcommand that speaks ACP",
                        manifest.id
                    );
                }
            }
        }
    }

    #[test]
    fn a_native_builtin_pins_nothing_and_needs_no_npm() {
        let suzent = builtins()
            .into_iter()
            .find(|e| e.manifest.id == "suzent")
            .expect("suzent is a built-in");
        let manifest = suzent.manifest;
        assert_eq!(manifest.kind, Kind::Native);
        assert!(manifest.package.is_empty(), "nothing to fetch from npm");
        assert!(manifest.version.is_empty(), "nothing to pin");
        validate_manifest(&manifest).unwrap();
    }

    // The guards that keep the two kinds from being described in each other's
    // terms, since a mismatch would silently produce a broken install path.
    #[test]
    fn a_native_manifest_must_not_claim_a_package_or_version() {
        let mut manifest = builtins()
            .into_iter()
            .find(|e| e.manifest.id == "suzent")
            .unwrap()
            .manifest;
        manifest.package = "some-package".into();
        assert!(validate_manifest(&manifest).is_err());

        let mut manifest = builtins()
            .into_iter()
            .find(|e| e.manifest.id == "suzent")
            .unwrap()
            .manifest;
        manifest.version = "1.0.0".into();
        assert!(validate_manifest(&manifest).is_err());
    }

    #[test]
    fn an_npm_manifest_must_not_carry_args() {
        let mut manifest = builtins()
            .into_iter()
            .find(|e| e.manifest.kind == Kind::Npm)
            .unwrap()
            .manifest;
        manifest.args = vec!["acp".into()];
        assert!(validate_manifest(&manifest).is_err());
    }

    // A native handle follows PATH by name so reinstalling the CLI under a
    // different prefix cannot strand it; an npm adapter keeps its pinned path.
    #[test]
    fn a_native_launch_command_carries_the_acp_subcommand() {
        let suzent = builtins()
            .into_iter()
            .find(|e| e.manifest.id == "suzent")
            .unwrap()
            .manifest;
        assert_eq!(
            suzent.launch_command(Path::new("suzent")),
            vec!["suzent".to_string(), "acp".to_string()]
        );
    }

    // Regression: the view and the installer must agree on the handle argv, or
    // a freshly-installed native handle reports itself as legacy-configured.
    #[test]
    fn a_native_handle_ignores_the_resolved_path_and_follows_path_by_name() {
        let suzent = builtins()
            .into_iter()
            .find(|e| e.manifest.id == "suzent")
            .unwrap()
            .manifest;
        // Whatever absolute path the probe resolved, the handle stays by-name.
        assert_eq!(
            suzent.handle_command(Path::new("/usr/local/bin/suzent")),
            vec!["suzent".to_string(), "acp".to_string()]
        );
    }

    #[test]
    fn an_npm_handle_keeps_its_pinned_absolute_path() {
        let claude = builtins()
            .into_iter()
            .find(|e| e.manifest.id == CLAUDE_PLUGIN_ID)
            .unwrap()
            .manifest;
        let pinned =
            Path::new("/adapters/claude-agent-acp/0.69.0/node_modules/.bin/claude-agent-acp");
        assert_eq!(
            claude.handle_command(pinned),
            vec![pinned.to_string_lossy().into_owned()]
        );
    }

    #[test]
    fn a_native_plugin_is_ready_exactly_when_its_cli_resolves() {
        let suzent = builtins()
            .into_iter()
            .find(|e| e.manifest.id == "suzent")
            .unwrap()
            .manifest;
        let expected = if super::super::probe::is_installed(&suzent.binary) {
            PluginState::Ready
        } else {
            PluginState::Missing
        };
        // The base dir is irrelevant for a native plugin: nothing is installed
        // under it, so state must not depend on it at all.
        assert_eq!(state_at(Path::new("/nonexistent"), &suzent), expected);
    }

    #[test]
    fn parses_node_major_versions() {
        assert_eq!(node_major("v22.14.0"), Some(22));
        assert_eq!(node_major("24.1.2\n"), Some(24));
        assert_eq!(node_major("not-a-version"), None);
    }

    #[test]
    fn package_managers_are_not_runtime_ready() {
        assert_eq!(
            command_status(&["npx".into(), "pkg".into()]),
            "runtime_download"
        );
        assert_eq!(command_status(&[]), "missing");
    }

    #[test]
    fn rejects_version_ranges_and_path_ids() {
        let mut manifest = builtins()[0].manifest.clone();
        manifest.version = "^1.0".into();
        assert!(validate_manifest(&manifest).is_err());
        manifest.version = "1.0.0".into();
        manifest.id = "../escape".into();
        assert!(validate_manifest(&manifest).is_err());
    }
}
