//! Managed agent-adapter plugins.
//!
//! A chat mention must never depend on a live package-manager download. Plugin
//! installation is an explicit control-plane action; after that, the reaction
//! path launches a version-pinned executable from `~/.enoxian/adapters`.
//!
//! Enoxian ships a small built-in catalog and also loads additional TOML
//! manifests from `~/.enoxian/plugins/*.toml`. Third-party manifests use the
//! same installer and health model as built-ins, but cannot override built-in
//! plugin ids. Below both sit the entries of the public ACP Registry (see
//! [`super::acp_registry`]): first the ones installed here, frozen at the
//! version installed, then the rest of the cached index.

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
    /// A prebuilt release Enoxian downloads, checks against its published
    /// SHA-256 and unpacks under `~/.enoxian/adapters`. Needs no runtime.
    Binary,
    /// A pinned PyPI package Enoxian installs into its own directory with
    /// `uv tool install`. Needs `uv` on PATH.
    Uvx,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Npm => "npm",
            Kind::Native => "native",
            Kind::Binary => "binary",
            Kind::Uvx => "uvx",
        }
    }
}

/// Where a registry plugin came from, shown so the user can judge it before
/// installing third-party code.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryInfo {
    pub name: String,
    #[serde(default)]
    pub license: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub website: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    /// Executable name exposed in `node_modules/.bin`; for [`Kind::Native`]
    /// the CLI name looked up on `PATH`; for [`Kind::Binary`] the executable's
    /// relative path inside the unpacked release; for [`Kind::Uvx`] the
    /// script name in the tool's bin directory.
    pub binary: String,
    #[serde(default)]
    pub kind: Kind,
    /// Arguments appended after `binary` when launching. A native CLI needs the
    /// subcommand that speaks ACP (`suzent acp`), as do product CLIs installed
    /// from the registry (`gemini --acp`); a pure adapter needs none.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment the launched process gets, copied into the chat handle.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// [`Kind::Binary`] only: the release archive for this platform.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub archive: String,
    /// [`Kind::Binary`] only: the archive's published SHA-256, lowercase hex.
    /// Empty when the publisher gave none; installing then needs consent.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sha256: String,
    /// Where to get the CLI, shown when a native plugin's binary is missing.
    #[serde(default)]
    pub install_url: String,
    #[serde(default)]
    pub about: String,
    /// Set for plugins that came from the ACP Registry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<RegistryInfo>,
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
    /// "npm", "native", "binary" or "uvx".
    pub kind: String,
    /// Whether this plugin needs system Node.js at all. False for a native
    /// plugin, so the UI must not demand a runtime it never uses.
    pub requires_node: bool,
    /// Whether this plugin needs `uv` (uvx packages), and whether it is here.
    pub requires_uv: bool,
    pub uv_installed: bool,
    /// "builtin", "local" (a manifest in `~/.enoxian/plugins`) or "registry".
    pub origin: String,
    /// Registry display name, license and links; `None` for other origins.
    pub registry: Option<RegistryInfo>,
    /// The registry's current version when it differs from the one this
    /// device has installed (an upgrade is available).
    pub update_version: Option<String>,
    /// For an update that is a prebuilt release: whether its publisher gave a
    /// checksum. Installing an unverified one needs explicit consent.
    pub update_verified: Option<bool>,
    /// For a prebuilt release: whether its publisher gave a checksum.
    pub verified: Option<bool>,
    /// Extra environment the handle launches with.
    pub env: BTreeMap<String, String>,
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
    pub origin: Origin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Builtin,
    /// A manifest file in `~/.enoxian/plugins`.
    Local,
    /// Installed on this device from the ACP Registry.
    RegistryInstalled,
    /// In the cached ACP Registry index, not installed.
    Registry,
}

impl Origin {
    pub fn is_registry(self) -> bool {
        matches!(self, Origin::Registry | Origin::RegistryInstalled)
    }
}

const CODEX_VERSION: &str = "1.1.14";
const PI_VERSION: &str = "0.0.33";
const CLAUDE_VERSION: &str = "0.112.0";
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
                ..Default::default()
            },
            source: "builtin".into(),
            origin: Origin::Builtin,
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
                ..Default::default()
            },
            source: "builtin".into(),
            origin: Origin::Builtin,
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
                ..Default::default()
            },
            source: "builtin".into(),
            origin: Origin::Builtin,
        },
        CatalogEntry {
            manifest: PluginManifest {
                id: "pi-acp".into(),
                agent: "pi".into(),
                version: PI_VERSION.into(),
                driver: Driver::Acp,
                package: "pi-acp".into(),
                binary: "pi-acp".into(),
                kind: Kind::Npm,
                args: Vec::new(),
                install_url: String::new(),
                about: "Pi coding agent through a pinned ACP adapter. Authenticate pi first \
                        (run `pi`, then `/login`, or export a provider API key)."
                    .into(),
                ..Default::default()
            },
            source: "builtin".into(),
            origin: Origin::Builtin,
        },
        CatalogEntry {
            manifest: PluginManifest {
                id: "hermes".into(),
                agent: "hermes".into(),
                version: String::new(),
                driver: Driver::Acp,
                package: String::new(),
                binary: "hermes".into(),
                kind: Kind::Native,
                args: vec!["acp".into()],
                install_url: "https://hermes-agent.nousresearch.com/docs/user-guide/features/acp"
                    .into(),
                about: "Hermes Agent speaking ACP itself — no adapter, no Node.js. Needs the \
                        ACP extra installed (`uv pip install -e '.[acp]'`) and providers \
                        configured with `hermes model`."
                    .into(),
                ..Default::default()
            },
            source: "builtin".into(),
            origin: Origin::Builtin,
        },
        CatalogEntry {
            manifest: PluginManifest {
                id: "openclaw".into(),
                agent: "openclaw".into(),
                version: String::new(),
                driver: Driver::Acp,
                package: String::new(),
                binary: "openclaw".into(),
                kind: Kind::Native,
                args: vec!["acp".into()],
                install_url: "https://docs.openclaw.ai/cli/acp".into(),
                about: "OpenClaw's own ACP bridge — no adapter, no Node.js. Needs an OpenClaw \
                        Gateway reachable, since the bridge forwards every turn to it."
                    .into(),
                ..Default::default()
            },
            source: "builtin".into(),
            origin: Origin::Builtin,
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

/// Built-ins, then valid third-party manifests, then registry plugins
/// installed here, then the rest of the cached registry. Invalid manifests are
/// ignored with a warning so one plugin cannot break the settings page.
///
/// An earlier tier wins on a clashing id, and a registry entry is also hidden
/// when an earlier tier already installs the same npm package: the registry's
/// `claude-acp` is the built-in Claude bridge under another name.
pub fn catalog() -> Vec<CatalogEntry> {
    let live: Vec<PluginManifest> = super::acp_registry::cached()
        .iter()
        .filter_map(super::acp_registry::to_manifest)
        .collect();
    catalog_from(local_manifests(), super::acp_registry::installed(), live)
}

fn local_manifests() -> Vec<(PluginManifest, PathBuf)> {
    let Ok(dir) = plugins_dir() else {
        return Vec::new();
    };
    let Ok(read) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for file in read.flatten() {
        let path = file.path();
        if path.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let parsed = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))
            .and_then(|text| {
                toml::from_str::<PluginManifest>(&text).context("invalid plugin manifest")
            });
        match parsed {
            Ok(manifest) => out.push((manifest, path)),
            Err(e) => tracing::warn!("[plugin] skipping {}: {e:#}", path.display()),
        }
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

fn catalog_from(
    local: Vec<(PluginManifest, PathBuf)>,
    installed: Vec<(PluginManifest, PathBuf)>,
    live: Vec<PluginManifest>,
) -> Vec<CatalogEntry> {
    let mut entries: Vec<CatalogEntry> = builtins();
    let add = |entries: &mut Vec<CatalogEntry>,
               manifest: PluginManifest,
               source: String,
               origin: Origin| {
        if let Err(e) = validate_manifest(&manifest) {
            tracing::warn!("[plugin] skipping '{}' from {source}: {e:#}", manifest.id);
            return;
        }
        if entries.iter().any(|e| e.manifest.id == manifest.id) {
            if origin != Origin::Registry {
                tracing::warn!(
                    "[plugin] duplicate id '{}' in {source}; built-in/first entry wins",
                    manifest.id
                );
            }
            return;
        }
        let same_package = |e: &CatalogEntry| {
            manifest.kind == Kind::Npm
                && e.manifest.kind == Kind::Npm
                && e.manifest.package == manifest.package
                && !e.origin.is_registry()
        };
        if origin.is_registry() && entries.iter().any(same_package) {
            return;
        }
        entries.push(CatalogEntry {
            manifest,
            source,
            origin,
        });
    };
    for (manifest, path) in local {
        add(
            &mut entries,
            manifest,
            path.to_string_lossy().into_owned(),
            Origin::Local,
        );
    }
    for (manifest, path) in installed {
        add(
            &mut entries,
            manifest,
            path.to_string_lossy().into_owned(),
            Origin::RegistryInstalled,
        );
    }
    for manifest in live {
        add(
            &mut entries,
            manifest,
            super::acp_registry::REGISTRY_URL.into(),
            Origin::Registry,
        );
    }
    // Registry entries after everything else, each group by id.
    entries.sort_by(|a, b| {
        (a.origin.is_registry(), &a.manifest.id).cmp(&(b.origin.is_registry(), &b.manifest.id))
    });
    entries
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

fn is_slug(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// An exact version, usable as a directory name.
fn validate_version(version: &str) -> Result<()> {
    if version.is_empty()
        || version == "latest"
        || version == "."
        || version == ".."
        || version.chars().any(|c| {
            c.is_whitespace()
                || matches!(
                    c,
                    '*' | '^' | '~' | '<' | '>' | '=' | '!' | ',' | '|' | '/' | '\\'
                )
        })
    {
        bail!("version must be exact (not latest or a range)");
    }
    Ok(())
}

fn validate_manifest(m: &PluginManifest) -> Result<()> {
    for (label, value) in [("id", &m.id), ("agent", &m.agent)] {
        if !is_slug(value) {
            bail!("{label} must contain only letters, digits, '-', '_' or '.'");
        }
    }
    for key in m.env.keys() {
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            bail!("environment variable name '{key}' is not valid");
        }
    }
    match m.kind {
        // A native plugin has no package to fetch and no version to pin: the
        // CLI on PATH is whatever the user installed, so requiring either would
        // be a fiction. Everything else about it is validated the same way.
        Kind::Native => {
            if !is_slug(&m.binary) {
                bail!("binary must contain only letters, digits, '-', '_' or '.'");
            }
            if !m.package.trim().is_empty() {
                bail!("a native plugin installs nothing, so it must not name a package");
            }
            if !m.version.trim().is_empty() {
                bail!("a native plugin pins nothing, so it must not claim a version");
            }
        }
        Kind::Npm => {
            if !is_slug(&m.binary) {
                bail!("binary must contain only letters, digits, '-', '_' or '.'");
            }
            if m.package.trim().is_empty() || m.package.chars().any(char::is_whitespace) {
                bail!("package must be a non-empty npm package name without whitespace");
            }
            validate_version(&m.version)?;
        }
        Kind::Uvx => {
            if !is_slug(&m.binary) {
                bail!("binary must contain only letters, digits, '-', '_' or '.'");
            }
            if !is_slug(&m.package) {
                bail!("package must be a PyPI package name");
            }
            validate_version(&m.version)?;
        }
        Kind::Binary => {
            validate_version(&m.version)?;
            if !m.package.is_empty() {
                bail!("a prebuilt release must not name a package");
            }
            if !m.archive.starts_with("https://") {
                bail!("a prebuilt release must be downloaded over https");
            }
            if !m.sha256.is_empty() && !super::archive::is_sha256_hex(&m.sha256) {
                bail!("sha256 must be 64 hex characters");
            }
            super::archive::relative_inside(Path::new("root"), &m.binary)?;
        }
    }
    Ok(())
}

fn install_root(base: &Path, manifest: &PluginManifest) -> PathBuf {
    base.join(&manifest.id).join(&manifest.version)
}

fn executable_at(base: &Path, manifest: &PluginManifest) -> PathBuf {
    let root = install_root(base, manifest);
    match manifest.kind {
        // Follow PATH rather than freezing an absolute path: the user owns this
        // CLI, and reinstalling it under a different prefix must not strand the
        // configured handle. Fall back to the bare name when it is absent, so
        // the view still has something to show.
        Kind::Native => super::probe::resolve(&manifest.binary)
            .unwrap_or_else(|| PathBuf::from(&manifest.binary)),
        Kind::Npm => {
            let bin = root.join("node_modules").join(".bin");
            #[cfg(windows)]
            {
                bin.join(format!("{}.cmd", manifest.binary))
            }
            #[cfg(not(windows))]
            {
                bin.join(&manifest.binary)
            }
        }
        Kind::Uvx => {
            let bin = root.join("bin");
            #[cfg(windows)]
            {
                bin.join(format!("{}.exe", manifest.binary))
            }
            #[cfg(not(windows))]
            {
                bin.join(&manifest.binary)
            }
        }
        // Validated to stay inside the root; an unvalidated manifest gets a
        // path that cannot exist rather than one outside it.
        Kind::Binary => super::archive::relative_inside(&root, &manifest.binary)
            .unwrap_or_else(|_| root.join(".invalid")),
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

fn uv_installed() -> bool {
    super::probe::resolve("uv").is_some()
}

pub fn views() -> Vec<PluginView> {
    let cfg = AgentConfig::load();
    let base = adapters_dir().unwrap_or_default();
    let (node_runtime_installed, node_runtime_version) = system_node_status();
    let uv_installed = uv_installed();
    let live: Vec<PluginManifest> = super::acp_registry::cached()
        .iter()
        .filter_map(super::acp_registry::to_manifest)
        .collect();
    catalog_from(
        local_manifests(),
        super::acp_registry::installed(),
        live.clone(),
    )
    .into_iter()
    .map(|entry| {
        let manifest = entry.manifest;
        let executable = executable_at(&base, &manifest);
        let expected = manifest.handle_command(&executable);
        let configured_cmd = cfg.resolve(&manifest.agent);
        let native = manifest.kind == Kind::Native;
        let requires_node = manifest.kind == Kind::Npm;
        // A native plugin's "bridged CLI" is its own binary: there is no
        // adapter in front of it, so its presence is the prerequisite.
        let bridge = super::probe::bridged_cli(&manifest.binary);
        let update = (entry.origin == Origin::RegistryInstalled)
            .then(|| live.iter().find(|m| m.id == manifest.id))
            .flatten()
            .filter(|m| m.version != manifest.version || m.kind != manifest.kind);
        // A registry entry's handle is only a default: a custom agent that
        // happens to share it is the user's own, not a stale install of this
        // plugin, so it must not mark the entry configured or migratable.
        let configured_cmd = configured_cmd
            .filter(|c| handle_belongs(entry.origin, &base, &manifest.id, &expected, &c.command));
        PluginView {
            id: manifest.id.clone(),
            agent: manifest.agent.clone(),
            version: manifest.version.clone(),
            driver: format!("{:?}", manifest.driver).to_lowercase(),
            kind: manifest.kind.as_str().into(),
            requires_node,
            requires_uv: manifest.kind == Kind::Uvx,
            uv_installed,
            origin: match entry.origin {
                Origin::Builtin => "builtin",
                Origin::Local => "local",
                Origin::Registry | Origin::RegistryInstalled => "registry",
            }
            .into(),
            registry: manifest.registry.clone(),
            update_version: update.map(|m| m.version.clone()),
            update_verified: update
                .filter(|m| m.kind == Kind::Binary)
                .map(|m| !m.sha256.is_empty()),
            verified: (manifest.kind == Kind::Binary).then_some(!manifest.sha256.is_empty()),
            env: manifest.env.clone(),
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
            node_runtime_installed: if requires_node {
                node_runtime_installed
            } else {
                true
            },
            node_runtime_version: if requires_node {
                node_runtime_version.clone()
            } else {
                None
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

/// Choices for [`install_with`]. The defaults install (or repair) what the
/// catalog shows.
#[derive(Debug, Clone, Default)]
pub struct InstallOptions {
    /// Chat handle to use instead of the plugin's default. Registry plugins
    /// only: a built-in's handle is part of the product.
    pub handle: Option<String>,
    /// For a registry plugin installed here: install the registry's current
    /// version instead of repairing the installed one.
    pub upgrade: bool,
    /// Accept a prebuilt release whose publisher gave no checksum.
    pub allow_unverified: bool,
}

/// Install a pinned adapter and configure its chat handle to launch the exact
/// managed executable. This is the only networked phase; mention execution is
/// offline and deterministic afterwards.
pub async fn install(id: &str) -> Result<AgentCommand> {
    install_with(id, &InstallOptions::default()).await
}

/// What an install acts on: the manifest (with the handle it will configure),
/// where it came from, and the handle it was installed under before, if any.
struct Plan {
    manifest: PluginManifest,
    origin: Origin,
    previous_handle: Option<String>,
}

fn plan_install(id: &str, opts: &InstallOptions) -> Result<Plan> {
    let entry = find(id).with_context(|| format!("unknown agent plugin '{id}'"))?;
    let origin = entry.origin;
    let mut manifest = entry.manifest;
    let previous_handle = (origin == Origin::RegistryInstalled).then(|| manifest.agent.clone());
    if opts.upgrade {
        if origin != Origin::RegistryInstalled {
            bail!(
                "'{id}' is not a registry plugin installed on this device; \
                 built-in plugins are updated with Enoxian itself"
            );
        }
        let live = super::acp_registry::cached()
            .iter()
            .filter_map(super::acp_registry::to_manifest)
            .find(|m| m.id == manifest.id)
            .with_context(|| {
                format!("'{id}' is no longer in the ACP registry for this platform")
            })?;
        manifest = PluginManifest {
            agent: manifest.agent,
            ..live
        };
    }
    if let Some(handle) = &opts.handle {
        if !origin.is_registry() {
            bail!("only registry plugins can be installed under another handle");
        }
        manifest.agent = handle.trim().trim_start_matches('@').to_string();
    }
    validate_manifest(&manifest)?;
    if manifest.kind == Kind::Binary && manifest.sha256.is_empty() && !opts.allow_unverified {
        bail!(
            "the publisher of '{}' gives no checksum for this platform's release, so it \
             cannot be verified; install it anyway with --allow-unverified",
            manifest.id
        );
    }
    if origin.is_registry() {
        check_handle_free(&manifest, previous_handle.as_deref())?;
    }
    Ok(Plan {
        manifest,
        origin,
        previous_handle,
    })
}

/// A registry plugin must not take over a handle that already means something
/// else on this device: a built-in's, another plugin's, or one the user set up.
fn check_handle_free(manifest: &PluginManifest, previous: Option<&str>) -> Result<()> {
    let handle = &manifest.agent;
    if let Some(other) = catalog().into_iter().find(|e| {
        e.manifest.id != manifest.id && e.origin != Origin::Registry && &e.manifest.agent == handle
    }) {
        bail!(
            "@{handle} already belongs to plugin '{}'; choose another handle with --as",
            other.manifest.id
        );
    }
    let cfg = AgentConfig::load();
    if let Some(existing) = cfg.resolve(handle) {
        if previous != Some(handle.as_str()) && !launches_from(&existing.command, &manifest.id) {
            bail!(
                "@{handle} is already configured on this device ({}); choose another \
                 handle with --as, or remove it first with `enox agent remove {handle}`",
                existing.command.join(" ")
            );
        }
    }
    Ok(())
}

/// Whether a configured command launches something installed for plugin `id`.
fn launches_from(command: &[String], id: &str) -> bool {
    adapters_dir()
        .map(|base| launches_from_at(&base, command, id))
        .unwrap_or(false)
}

fn launches_from_at(base: &Path, command: &[String], id: &str) -> bool {
    command
        .first()
        .is_some_and(|program| Path::new(program).starts_with(base.join(id)))
}

/// Whether the agent configured under a plugin's handle is that plugin's.
/// Built-in and local handles are part of the plugin. A registry handle is
/// only a default, so a custom agent that shares it stays the user's own.
fn handle_belongs(
    origin: Origin,
    base: &Path,
    id: &str,
    expected: &[String],
    command: &[String],
) -> bool {
    !origin.is_registry() || command == expected || launches_from_at(base, command, id)
}

/// Run an installer command, turning a failure into the tail of its stderr.
async fn run_installer(mut command: tokio::process::Command, what: &str) -> Result<()> {
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = command
        .output()
        .await
        .with_context(|| format!("failed to start {what}"))?;
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
        bail!("{what} failed ({}): {}", output.status, detail.trim());
    }
    Ok(())
}

pub async fn install_with(id: &str, opts: &InstallOptions) -> Result<AgentCommand> {
    let Plan {
        mut manifest,
        origin,
        previous_handle,
    } = plan_install(id, opts)?;
    if manifest.kind == Kind::Native {
        return configure_native(&manifest);
    }
    if manifest.kind == Kind::Npm {
        if let Some(bridge) = super::probe::bridged_cli(&manifest.binary) {
            verify_bridged_runtime(bridge).await?;
        }
    }
    let npm = match manifest.kind {
        Kind::Npm => Some(require_system_npm().await?),
        _ => None,
    };
    let uv = match manifest.kind {
        Kind::Uvx => Some(super::probe::resolve("uv").context(
            "this agent is a Python package and needs uv; install it from \
             https://docs.astral.sh/uv/ and try again",
        )?),
        _ => None,
    };
    let base = adapters_dir()?;
    let root = install_root(&base, &manifest);
    std::fs::create_dir_all(&root)?;
    let lock = root.join(".installing");
    // A killed daemon must not leave this plugin permanently unrepairable.
    // A live install is bounded to twenty minutes by the API, so a marker
    // older than half an hour cannot belong to an active supported install.
    if lock.exists() {
        let stale = lock
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .map(|age| age > Duration::from_secs(30 * 60))
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

    match manifest.kind {
        Kind::Npm => {
            let npm = npm.expect("resolved above");
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
            let command = super::spawn::command(&npm.to_string_lossy(), &args);
            run_installer(command, "npm install").await?;
            // The registry names only the package; its `bin` map says what
            // the executable is called.
            if origin.is_registry() {
                manifest.binary = resolve_npm_bin(&root, &manifest)?;
            }
        }
        Kind::Uvx => {
            let uv = uv.expect("resolved above");
            let args = vec![
                "tool".into(),
                "install".into(),
                "--force".into(),
                "--quiet".into(),
                format!("{}=={}", manifest.package, manifest.version),
            ];
            let mut command = super::spawn::command(&uv.to_string_lossy(), &args);
            // Keep the tool and its scripts inside this version's directory,
            // out of the user's own `uv tool` set.
            command
                .env("UV_TOOL_DIR", root.join("tools"))
                .env("UV_TOOL_BIN_DIR", root.join("bin"));
            run_installer(command, "uv tool install").await?;
            if !executable_at(&base, &manifest).is_file() {
                manifest.binary = sole_script(&root.join("bin"))?;
            }
        }
        Kind::Binary => install_release(&root, &manifest).await?,
        Kind::Native => unreachable!("handled above"),
    }

    let exe = executable_at(&base, &manifest);
    if !exe.is_file() {
        bail!("plugin installed but '{}' was not created", exe.display());
    }
    if origin.is_registry() {
        super::acp_registry::freeze(&manifest)?;
        // Moving to another handle frees the old one.
        if let Some(old) = previous_handle.filter(|old| old != &manifest.agent) {
            let mut cfg = AgentConfig::load_for_edit()?;
            if cfg
                .resolve(&old)
                .is_some_and(|c| launches_from(&c.command, &manifest.id))
            {
                cfg.remove_agent(&old);
                cfg.save()?;
            }
        }
    }
    configure_handle(&manifest, &exe)
}

/// What [`uninstall`] removed.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Uninstalled {
    /// The chat handle that was removed, when it launched this plugin.
    pub handle: Option<String>,
    /// Whether downloaded files were deleted.
    pub files: bool,
}

/// Remove a plugin from this device: its chat handle (only when the handle
/// still launches this plugin's install), everything downloaded for it, and —
/// for a registry plugin — the record of what was installed.
///
/// A native plugin's CLI belongs to the user and is never touched; only its
/// handle goes.
pub fn uninstall(id: &str) -> Result<Uninstalled> {
    let entry = find(id).with_context(|| format!("unknown agent plugin '{id}'"))?;
    let manifest = entry.manifest;
    let mut out = Uninstalled::default();
    let mut cfg = AgentConfig::load_for_edit()?;
    let base = adapters_dir()?;
    let expected = manifest.handle_command(&executable_at(&base, &manifest));
    if let Some(existing) = cfg.resolve(&manifest.agent) {
        if existing.command == expected || launches_from(&existing.command, &manifest.id) {
            cfg.remove_agent(&manifest.agent);
            cfg.save()?;
            out.handle = Some(manifest.agent.clone());
        }
    }
    if manifest.kind != Kind::Native {
        let dir = base.join(&manifest.id);
        if dir.join(&manifest.version).join(".installing").exists() {
            bail!(
                "plugin '{}' is installing; try again when it finishes",
                manifest.id
            );
        }
        if dir.exists() {
            std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
            out.files = true;
        }
    }
    if entry.origin == Origin::RegistryInstalled {
        super::acp_registry::unfreeze(&manifest.id)?;
    }
    Ok(out)
}

/// Download, verify and unpack a prebuilt release into `root`.
async fn install_release(root: &Path, manifest: &PluginManifest) -> Result<()> {
    // Start from an empty directory so a half-unpacked earlier attempt
    // cannot mix with this one. Only the install marker survives.
    for entry in std::fs::read_dir(root)?.flatten() {
        let path = entry.path();
        if path.file_name().and_then(|n| n.to_str()) == Some(".installing") {
            continue;
        }
        if path.is_dir() && !path.is_symlink() {
            std::fs::remove_dir_all(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
    }
    let download = root.join(".download");
    let expected = (!manifest.sha256.is_empty()).then_some(manifest.sha256.as_str());
    super::archive::download(&manifest.archive, &download, expected).await?;
    let exe = super::archive::relative_inside(root, &manifest.binary)?;
    let (from, into, target) = (download.clone(), root.to_path_buf(), exe.clone());
    tokio::task::spawn_blocking(move || super::archive::extract(&from, &into, &target))
        .await
        .context("unpacking the release")??;
    let _ = std::fs::remove_file(&download);
    super::archive::ensure_executable(&exe)
}

/// The executable an installed npm package provides, from its `bin` map.
fn resolve_npm_bin(root: &Path, manifest: &PluginManifest) -> Result<String> {
    let mut dir = root.join("node_modules");
    for part in manifest.package.split('/') {
        dir.push(part);
    }
    let text = std::fs::read_to_string(dir.join("package.json"))
        .with_context(|| format!("reading package.json of {}", manifest.package))?;
    let package: serde_json::Value = serde_json::from_str(&text)?;
    pick_npm_bin(&manifest.package, &manifest.binary, &package["bin"])
}

fn pick_npm_bin(package: &str, guess: &str, bin: &serde_json::Value) -> Result<String> {
    let basename = package.rsplit('/').next().unwrap_or(package);
    let names: Vec<String> = match bin {
        // A string `bin` is installed under the unscoped package name.
        serde_json::Value::String(_) => vec![basename.to_string()],
        serde_json::Value::Object(map) => map.keys().cloned().collect(),
        _ => Vec::new(),
    };
    let chosen = match names.as_slice() {
        [] => bail!("{package} does not provide an executable"),
        [only] => only.clone(),
        many => many
            .iter()
            .find(|n| *n == guess || *n == basename)
            .or_else(|| many.iter().find(|n| n.contains("acp")))
            .cloned()
            .with_context(|| {
                format!(
                    "{package} provides several executables ({}) and none is clearly its agent",
                    many.join(", ")
                )
            })?,
    };
    if !is_slug(&chosen) {
        bail!("{package} names an unusable executable '{chosen}'");
    }
    Ok(chosen)
}

/// The only script in a `uv tool` bin directory, when it is not named after
/// the package.
fn sole_script(bin: &Path) -> Result<String> {
    let names: Vec<String> = std::fs::read_dir(bin)
        .with_context(|| format!("reading {}", bin.display()))?
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let stem = path.file_stem()?.to_str()?.to_string();
            Some(stem)
        })
        .collect();
    match names.as_slice() {
        [only] if is_slug(only) => Ok(only.clone()),
        _ => bail!(
            "the package installed {} executables ({}), and none is named after it",
            names.len(),
            names.join(", ")
        ),
    }
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
        env: manifest.env.clone(),
        ..Default::default()
    }
    .inheriting_delegation(cfg.resolve(&manifest.agent));
    cfg.set_agent(&manifest.agent, command.clone());
    cfg.save()?;
    Ok(command)
}

/// A bridge cannot work without the CLI it bridges to, so refuse to install one
/// whose CLI is absent. Authentication happens separately in device settings,
/// after the adapter is available to advertise its sign-in methods.
async fn verify_bridged_runtime(bridge: &super::probe::BridgedCli) -> Result<()> {
    let _cli = super::probe::resolve(bridge.program).with_context(|| {
        format!(
            "the {} CLI is required but was not found on PATH. Install it from {}, then run `{}`",
            bridge.program, bridge.install_url, bridge.login_command
        )
    })?;

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
                Kind::Binary | Kind::Uvx => {
                    panic!("{} is a registry kind, not a built-in one", manifest.id)
                }
            }
        }
    }

    // The chat handles the built-in catalog offers, and how each one is
    // reached. These are the strings a user types as `@name`, so a rename is a
    // user-visible change rather than an internal one.
    #[test]
    fn built_in_handles_and_kinds_are_stable() {
        let kinds: Vec<(String, String, Kind)> = builtins()
            .into_iter()
            .map(|e| (e.manifest.id, e.manifest.agent, e.manifest.kind))
            .collect();
        for (id, agent, kind) in [
            ("codex-acp", "codex", Kind::Npm),
            ("claude-agent-acp", "claude", Kind::Npm),
            ("pi-acp", "pi", Kind::Npm),
            ("suzent", "suzent", Kind::Native),
            ("hermes", "hermes", Kind::Native),
            ("openclaw", "openclaw", Kind::Native),
        ] {
            assert!(
                kinds.contains(&(id.to_string(), agent.to_string(), kind)),
                "{id} should be a built-in @{agent} of kind {kind:?}"
            );
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

    // Registry product CLIs need their ACP flag (`gemini --acp`), so an npm
    // plugin may carry args; the handle launches the pinned path plus them.
    #[test]
    fn an_npm_manifest_may_carry_args_into_its_handle() {
        let mut manifest = builtins()
            .into_iter()
            .find(|e| e.manifest.kind == Kind::Npm)
            .unwrap()
            .manifest;
        manifest.args = vec!["--acp".into()];
        validate_manifest(&manifest).unwrap();
        let exe = Path::new("/adapters/x/1/node_modules/.bin/x");
        assert_eq!(
            manifest.handle_command(exe),
            vec![exe.to_string_lossy().into_owned(), "--acp".into()]
        );
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

    fn registry_npm(id: &str, package: &str, version: &str) -> PluginManifest {
        PluginManifest {
            id: id.into(),
            agent: super::super::acp_registry::default_handle(id),
            version: version.into(),
            package: package.into(),
            binary: package.rsplit('/').next().unwrap().into(),
            kind: Kind::Npm,
            registry: Some(RegistryInfo::default()),
            ..Default::default()
        }
    }

    #[test]
    fn registry_entries_never_shadow_earlier_tiers() {
        let entries = catalog_from(
            Vec::new(),
            vec![(
                registry_npm("gemini", "@google/gemini-cli", "0.70.0"),
                PathBuf::from("installed/gemini.toml"),
            )],
            vec![
                // Same id as a built-in.
                registry_npm("codex-acp", "@agentclientprotocol/codex-acp", "2.12.1"),
                // Same package as the built-in Claude bridge, another id.
                registry_npm(
                    "claude-acp",
                    "@agentclientprotocol/claude-agent-acp",
                    "0.113.0",
                ),
                // Installed here already: the frozen copy wins.
                registry_npm("gemini", "@google/gemini-cli", "0.71.1"),
                registry_npm("cline", "cline", "3.0.76"),
            ],
        );
        let find = |id: &str| entries.iter().find(|e| e.manifest.id == id);
        assert_eq!(find("codex-acp").unwrap().origin, Origin::Builtin);
        assert!(find("claude-acp").is_none());
        let gemini = find("gemini").unwrap();
        assert_eq!(gemini.origin, Origin::RegistryInstalled);
        assert_eq!(gemini.manifest.version, "0.70.0");
        assert_eq!(find("cline").unwrap().origin, Origin::Registry);
        // Registry entries come after the rest.
        let first_registry = entries.iter().position(|e| e.origin.is_registry()).unwrap();
        assert!(entries[first_registry..]
            .iter()
            .all(|e| e.origin.is_registry()));
    }

    #[test]
    fn picks_the_agent_executable_from_an_npm_bin_map() {
        use serde_json::json;
        assert_eq!(
            pick_npm_bin(
                "@google/gemini-cli",
                "gemini-cli",
                &json!({"gemini": "dist/index.js"})
            )
            .unwrap(),
            "gemini"
        );
        assert_eq!(
            pick_npm_bin("@scope/tool", "tool", &json!("cli.js")).unwrap(),
            "tool"
        );
        assert_eq!(
            pick_npm_bin("cline", "cline", &json!({"cline": "a", "cline-host": "b"})).unwrap(),
            "cline"
        );
        assert_eq!(
            pick_npm_bin("pkg", "pkg", &json!({"helper": "a", "pkg-acp": "b"})).unwrap(),
            "pkg-acp"
        );
        assert!(pick_npm_bin("pkg", "pkg", &json!({"a": "1", "b": "2"})).is_err());
        assert!(pick_npm_bin("pkg", "pkg", &json!(null)).is_err());
        assert!(pick_npm_bin("pkg", "pkg", &json!({"../x": "1"})).is_err());
    }

    #[test]
    fn a_prebuilt_release_must_be_https_and_stay_in_its_directory() {
        let good = PluginManifest {
            id: "goose".into(),
            agent: "goose".into(),
            version: "1.61.1".into(),
            kind: Kind::Binary,
            binary: "bin/goose".into(),
            archive: "https://example.com/goose.tar.gz".into(),
            sha256: "a".repeat(64),
            ..Default::default()
        };
        validate_manifest(&good).unwrap();
        for bad in [
            PluginManifest {
                archive: "http://example.com/x".into(),
                ..good.clone()
            },
            PluginManifest {
                binary: "../../bin/sh".into(),
                ..good.clone()
            },
            PluginManifest {
                binary: "/bin/sh".into(),
                ..good.clone()
            },
            PluginManifest {
                sha256: "nothex".into(),
                ..good.clone()
            },
            PluginManifest {
                version: "../1".into(),
                ..good.clone()
            },
            PluginManifest {
                package: "x".into(),
                ..good.clone()
            },
        ] {
            assert!(validate_manifest(&bad).is_err(), "{bad:?}");
        }
        let path = executable_at(Path::new("/a"), &good)
            .to_string_lossy()
            .replace('\\', "/");
        assert_eq!(path, "/a/goose/1.61.1/bin/goose");
    }

    #[test]
    fn rejects_bad_environment_names() {
        let mut manifest = builtins()[0].manifest.clone();
        manifest.env.insert("OK_NAME".into(), "1".into());
        validate_manifest(&manifest).unwrap();
        manifest.env.insert("BAD=NAME".into(), "1".into());
        assert!(validate_manifest(&manifest).is_err());
    }

    #[test]
    fn a_custom_handle_does_not_belong_to_a_registry_entry() {
        let base = Path::new("/adapters");
        let expected = vec!["/adapters/gemini/0.71.1/node_modules/.bin/gemini".to_string()];
        let custom = vec!["/usr/local/bin/gemini".to_string(), "--acp".to_string()];
        let older = vec!["/adapters/gemini/0.70.0/node_modules/.bin/gemini".to_string()];
        for origin in [Origin::Registry, Origin::RegistryInstalled] {
            assert!(!handle_belongs(origin, base, "gemini", &expected, &custom));
            assert!(handle_belongs(origin, base, "gemini", &expected, &expected));
            // An earlier install of the same plugin is still its own.
            assert!(handle_belongs(origin, base, "gemini", &expected, &older));
        }
        // A built-in's handle is the built-in's whatever it launches, so an
        // old runtime-download command still shows as migratable.
        assert!(handle_belongs(
            Origin::Builtin,
            base,
            "codex-acp",
            &expected,
            &custom
        ));
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
