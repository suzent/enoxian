//! The public ACP Registry as a source of installable agent plugins.
//!
//! The registry (<https://github.com/agentclientprotocol/registry>) is one JSON
//! index of ACP agents with exact versions and how each is distributed. This
//! module only *reads* it: a cached copy feeds the plugin catalog, and the
//! entries it can express as a pinned npm install become ordinary
//! [`PluginManifest`]s. Installing one is still the explicit, user-initiated
//! step in [`super::plugin`]; a chat mention never touches the network.
//!
//! The index is fetched on demand (listing plugins, or `enox agent registry
//! refresh`) and cached under `~/.enoxian/registry/`. Every failure here leaves
//! the catalog as it was: a broken or unreachable registry costs the extra
//! entries, never the built-ins.
//!
//! Each entry becomes one manifest for *this* device: a prebuilt `binary` for
//! this platform when the publisher ships one (no runtime needed), otherwise
//! its `npx` package (pinned npm install), otherwise its `uvx` package (pinned
//! `uv tool` install). An entry with none of these for this platform is left
//! out.
//!
//! Installing freezes the manifest that was installed (see [`freeze`]), so a
//! later registry update shows as an available upgrade instead of silently
//! changing what an `@mention` launches.

use super::plugin::{Kind, PluginManifest, RegistryInfo};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const REGISTRY_URL: &str =
    "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";

/// How long a cached index is used before listing plugins refetches it.
const MAX_AGE: chrono::Duration = chrono::Duration::hours(24);
/// The index is about 75 KB today; anything near this is not the registry.
const MAX_BYTES: usize = 4 * 1024 * 1024;
/// Bounds the whole request, so a slow CDN cannot stall the settings page.
const FETCH_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Deserialize)]
struct Index {
    /// Format version of the index. Only major version 1 is understood.
    version: String,
    #[serde(default)]
    agents: Vec<Agent>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Agent {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub website: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub distribution: Distribution,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Distribution {
    #[serde(default)]
    pub npx: Option<Package>,
    #[serde(default)]
    pub uvx: Option<Package>,
    /// Keyed by platform, e.g. `darwin-aarch64` or `linux-x86_64-musl`.
    #[serde(default)]
    pub binary: BTreeMap<String, Binary>,
}

/// An `npx` or `uvx` distribution.
#[derive(Debug, Clone, Deserialize)]
pub struct Package {
    /// `name@exact-version` (npm, scoped or not) or `name==version` (PyPI).
    pub package: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// One platform's prebuilt release.
#[derive(Debug, Clone, Deserialize)]
pub struct Binary {
    pub archive: String,
    /// The executable's path inside the unpacked archive.
    pub cmd: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Meta {
    fetched_at: Option<chrono::DateTime<chrono::Utc>>,
    etag: Option<String>,
}

fn dir() -> Result<PathBuf> {
    Ok(crate::config::enoxian_dir()?.join("registry"))
}

fn index_path(dir: &Path) -> PathBuf {
    dir.join("acp-registry.json")
}

fn meta_path(dir: &Path) -> PathBuf {
    dir.join("acp-registry.meta.json")
}

fn parse(body: &[u8]) -> Result<Index> {
    let index: Index = serde_json::from_slice(body).context("not a registry index")?;
    if index.version.split('.').next() != Some("1") {
        bail!("unsupported registry format version {}", index.version);
    }
    Ok(index)
}

fn read_cached(dir: &Path) -> Option<Vec<Agent>> {
    let body = std::fs::read(index_path(dir)).ok()?;
    match parse(&body) {
        Ok(index) => Some(index.agents),
        Err(e) => {
            tracing::warn!("[registry] ignoring cached index: {e:#}");
            None
        }
    }
}

fn read_meta(dir: &Path) -> Meta {
    std::fs::read(meta_path(dir))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// The agents in the cached index; empty when there is none yet.
pub fn cached() -> Vec<Agent> {
    dir().ok().and_then(|d| read_cached(&d)).unwrap_or_default()
}

/// When the cached index was last confirmed current, if ever.
pub fn fetched_at() -> Option<chrono::DateTime<chrono::Utc>> {
    dir().ok().and_then(|d| read_meta(&d).fetched_at)
}

/// What a refresh did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refresh {
    /// The cache was young enough; nothing was requested.
    Fresh,
    /// The server confirmed the cached copy.
    NotModified,
    /// A new index was stored.
    Updated,
}

/// Refetch the index when the cache is older than a day, or always with
/// `force`. A conditional request keeps an unchanged index to one round trip.
pub async fn refresh(force: bool) -> Result<Refresh> {
    let dir = dir()?;
    std::fs::create_dir_all(&dir)?;
    let mut meta = read_meta(&dir);
    let have_index = index_path(&dir).is_file();
    let young = meta
        .fetched_at
        .map(|at| chrono::Utc::now() - at < MAX_AGE)
        .unwrap_or(false);
    if !force && have_index && young {
        return Ok(Refresh::Fresh);
    }

    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .user_agent(concat!("enoxian/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let mut request = client.get(REGISTRY_URL);
    if have_index {
        if let Some(etag) = &meta.etag {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag);
        }
    }
    let response = request.send().await.context("fetching the ACP registry")?;
    if response.status() == reqwest::StatusCode::NOT_MODIFIED && have_index {
        meta.fetched_at = Some(chrono::Utc::now());
        write_atomic(&meta_path(&dir), &serde_json::to_vec(&meta)?)?;
        return Ok(Refresh::NotModified);
    }
    if !response.status().is_success() {
        bail!("ACP registry returned {}", response.status());
    }
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    if response.content_length().unwrap_or(0) as usize > MAX_BYTES {
        bail!("ACP registry index is unexpectedly large");
    }
    let body = response.bytes().await.context("reading the ACP registry")?;
    if body.len() > MAX_BYTES {
        bail!("ACP registry index is unexpectedly large");
    }
    // Validate before replacing a good cache with a bad response.
    parse(&body)?;
    write_atomic(&index_path(&dir), &body)?;
    meta = Meta {
        fetched_at: Some(chrono::Utc::now()),
        etag,
    };
    write_atomic(&meta_path(&dir), &serde_json::to_vec(&meta)?)?;
    Ok(Refresh::Updated)
}

/// Refresh if stale, but never fail or wait long: used before listing plugins,
/// where an offline device should simply see the cached (or no) entries.
pub async fn refresh_quietly() {
    if let Err(e) = refresh(false).await {
        tracing::debug!("[registry] refresh skipped: {e:#}");
    }
}

/// Split `name@version` / `@scope/name@version` into its two halves.
fn split_package(spec: &str) -> Option<(&str, &str)> {
    let at = spec.rfind('@').filter(|&i| i > 0)?;
    let (name, version) = (&spec[..at], &spec[at + 1..]);
    (!name.is_empty() && !version.is_empty()).then_some((name, version))
}

/// Split a uvx spec: `name==version`, or the `name@version` uv also accepts.
fn split_python_package(spec: &str) -> Option<(&str, &str)> {
    let (name, version) = spec.split_once("==").or_else(|| spec.split_once('@'))?;
    let (name, version) = (name.trim(), version.trim());
    (!name.is_empty() && !version.is_empty()).then_some((name, version))
}

/// The chat handle a registry agent gets unless the user picks another:
/// its id without a trailing `-acp`, which only names the transport.
pub fn default_handle(id: &str) -> String {
    id.strip_suffix("-acp")
        .filter(|s| !s.is_empty())
        .unwrap_or(id)
        .to_string()
}

/// A provisional executable name, shown before install. The real one comes
/// from the installed package's `bin` map (see `plugin::resolve_npm_bin`),
/// since the registry names only the package.
fn guess_binary(package: &str) -> String {
    package.rsplit('/').next().unwrap_or(package).to_string()
}

/// Registry platform keys this device can run, best first.
pub fn platform_keys() -> Vec<String> {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "linux" => "linux",
        "windows" => "windows",
        _ => return Vec::new(),
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "aarch64",
        "x86_64" => "x86_64",
        _ => return Vec::new(),
    };
    let base = format!("{os}-{arch}");
    if os != "linux" {
        return vec![base];
    }
    // A glibc build needs the glibc loader; a musl build runs anywhere.
    let glibc = [
        "/lib64/ld-linux-x86-64.so.2",
        "/lib/ld-linux-aarch64.so.1",
        "/lib/x86_64-linux-gnu/libc.so.6",
        "/lib/aarch64-linux-gnu/libc.so.6",
    ]
    .iter()
    .any(|p| Path::new(p).exists());
    let musl = format!("{base}-musl");
    if glibc {
        vec![base, musl]
    } else {
        vec![musl]
    }
}

fn info(agent: &Agent) -> RegistryInfo {
    RegistryInfo {
        name: agent.name.clone(),
        license: agent.license.clone(),
        repository: agent.repository.clone(),
        website: agent.website.clone(),
        icon: agent.icon.clone(),
    }
}

fn about(agent: &Agent) -> String {
    let description = agent.description.trim();
    if description.is_empty() {
        agent.name.clone()
    } else {
        description.to_string()
    }
}

/// Express one registry entry as a manifest for this device, if it can be.
pub fn to_manifest(agent: &Agent) -> Option<PluginManifest> {
    to_manifest_for(agent, &platform_keys())
}

fn to_manifest_for(agent: &Agent, platforms: &[String]) -> Option<PluginManifest> {
    let base = PluginManifest {
        id: agent.id.clone(),
        agent: default_handle(&agent.id),
        driver: super::config::Driver::Acp,
        about: about(agent),
        registry: Some(info(agent)),
        ..Default::default()
    };
    if let Some(release) = platforms
        .iter()
        .find_map(|key| agent.distribution.binary.get(key))
    {
        return Some(PluginManifest {
            version: agent.version.clone(),
            kind: Kind::Binary,
            binary: release.cmd.trim_start_matches("./").to_string(),
            archive: release.archive.clone(),
            sha256: release
                .sha256
                .clone()
                .unwrap_or_default()
                .to_ascii_lowercase(),
            args: release.args.clone(),
            env: release.env.clone(),
            ..base
        });
    }
    if let Some(npx) = &agent.distribution.npx {
        let (package, version) = split_package(&npx.package)?;
        return Some(PluginManifest {
            version: version.to_string(),
            kind: Kind::Npm,
            package: package.to_string(),
            binary: guess_binary(package),
            args: npx.args.clone(),
            env: npx.env.clone(),
            ..base
        });
    }
    if let Some(uvx) = &agent.distribution.uvx {
        let (package, version) = split_python_package(&uvx.package)?;
        // `uvx <pkg>` runs the executable named after the package.
        let package = package.split('[').next().unwrap_or(package).to_string();
        return Some(PluginManifest {
            version: version.to_string(),
            kind: Kind::Uvx,
            binary: package.clone(),
            package,
            args: uvx.args.clone(),
            env: uvx.env.clone(),
            ..base
        });
    }
    None
}

// ── Installed (frozen) manifests ──────────────────────────────────────────

fn installed_dir() -> Result<PathBuf> {
    Ok(dir()?.join("installed"))
}

/// Manifests of registry plugins installed on this device, exactly as they
/// were installed — including the handle the user chose.
pub fn installed() -> Vec<(PluginManifest, PathBuf)> {
    let Ok(dir) = installed_dir() else {
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
        match std::fs::read_to_string(&path)
            .map_err(anyhow::Error::from)
            .and_then(|t| toml::from_str::<PluginManifest>(&t).map_err(Into::into))
        {
            Ok(m) => out.push((m, path)),
            Err(e) => tracing::warn!("[registry] skipping {}: {e:#}", path.display()),
        }
    }
    out.sort_by(|a, b| a.0.id.cmp(&b.0.id));
    out
}

/// Record what was installed, so the catalog keeps launching it after the
/// registry moves on.
pub fn freeze(manifest: &PluginManifest) -> Result<PathBuf> {
    let dir = installed_dir()?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.toml", manifest.id));
    write_atomic(&path, toml::to_string_pretty(manifest)?.as_bytes())?;
    Ok(path)
}

/// Forget an installed registry plugin.
pub fn unfreeze(id: &str) -> Result<()> {
    let path = installed_dir()?.join(format!("{id}.toml"));
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "version": "1.0.0",
      "agents": [
        {"id": "gemini", "name": "Gemini CLI", "version": "0.71.1",
         "description": "Google's agent", "license": "Apache-2.0",
         "license_url": "x", "icon": "https://cdn.example/gemini.svg",
         "repository": "https://github.com/google-gemini/gemini-cli",
         "distribution": {"npx": {"package": "@google/gemini-cli@0.71.1", "args": ["--acp"]}}},
        {"id": "factory-droid", "name": "Factory Droid", "version": "1.21.1",
         "description": "", "license": "proprietary", "license_url": "x", "icon": "i",
         "distribution": {"npx": {"package": "droid@1.21.1",
           "args": ["exec", "--output-format", "acp-daemon"],
           "env": {"DROID_DISABLE_AUTO_UPDATE": "1"}}}},
        {"id": "goose", "name": "goose", "version": "1.61.1", "description": "d",
         "license": "Apache-2.0", "license_url": "x", "icon": "i",
         "distribution": {"binary": {
           "darwin-aarch64": {"archive": "https://x/goose.tar.bz2", "cmd": "./goose", "sha256": "AB"},
           "linux-x86_64-musl": {"archive": "https://x/goose-musl.tgz", "cmd": "./bin/goose", "args": ["acp"]}}}},
        {"id": "fast-agent", "name": "fast-agent", "version": "0.10.66", "description": "d",
         "license": "Apache-2.0", "license_url": "x", "icon": "i",
         "distribution": {"uvx": {"package": "fast-agent-acp==0.10.66", "args": ["-x"]}}},
        {"id": "future", "name": "f", "version": "1", "description": "d",
         "license": "MIT", "license_url": "x", "icon": "i",
         "distribution": {"oci": {"image": "x"}}}
      ],
      "extensions": []
    }"#;

    #[test]
    fn parses_the_index_and_maps_npx_entries() {
        let index = parse(SAMPLE.as_bytes()).unwrap();
        assert_eq!(index.agents.len(), 5);
        // A platform with no prebuilt release for goose.
        let platforms = vec!["windows-x86_64".to_string()];
        let mapped: Vec<_> = index
            .agents
            .iter()
            .filter_map(|a| to_manifest_for(a, &platforms))
            .collect();
        let ids: Vec<_> = mapped.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["gemini", "factory-droid", "fast-agent"]);

        let gemini = &mapped[0];
        assert_eq!(gemini.id, "gemini");
        assert_eq!(gemini.agent, "gemini");
        assert_eq!(gemini.package, "@google/gemini-cli");
        assert_eq!(gemini.version, "0.71.1");
        assert_eq!(gemini.args, vec!["--acp"]);
        assert_eq!(gemini.binary, "gemini-cli");
        assert_eq!(gemini.kind, Kind::Npm);
        assert_eq!(gemini.registry.as_ref().unwrap().name, "Gemini CLI");

        let droid = &mapped[1];
        assert_eq!(droid.package, "droid");
        assert_eq!(droid.env["DROID_DISABLE_AUTO_UPDATE"], "1");
        // An empty description falls back to the display name.
        assert_eq!(droid.about, "Factory Droid");
    }

    #[test]
    fn picks_this_platforms_release_in_preference_order() {
        let index = parse(SAMPLE.as_bytes()).unwrap();
        let goose = &index.agents[2];
        let mac = to_manifest_for(goose, &["darwin-aarch64".into()]).unwrap();
        assert_eq!(mac.kind, Kind::Binary);
        assert_eq!(mac.binary, "goose");
        assert_eq!(mac.archive, "https://x/goose.tar.bz2");
        assert_eq!(mac.sha256, "ab");
        assert_eq!(mac.version, "1.61.1");

        let musl =
            to_manifest_for(goose, &["linux-x86_64".into(), "linux-x86_64-musl".into()]).unwrap();
        assert_eq!(musl.archive, "https://x/goose-musl.tgz");
        assert_eq!(musl.binary, "bin/goose");
        assert_eq!(musl.args, vec!["acp"]);
        assert!(musl.sha256.is_empty());
    }

    #[test]
    fn maps_a_uvx_package() {
        let index = parse(SAMPLE.as_bytes()).unwrap();
        let fast = to_manifest_for(&index.agents[3], &[]).unwrap();
        assert_eq!(fast.kind, Kind::Uvx);
        assert_eq!(fast.package, "fast-agent-acp");
        assert_eq!(fast.binary, "fast-agent-acp");
        assert_eq!(fast.version, "0.10.66");
        assert_eq!(
            split_python_package("minion-code@0.1.44"),
            Some(("minion-code", "0.1.44"))
        );
    }

    #[test]
    fn rejects_an_unknown_major_format() {
        let body = SAMPLE.replacen("\"1.0.0\"", "\"2.0.0\"", 1);
        assert!(parse(body.as_bytes()).is_err());
    }

    #[test]
    fn splits_scoped_and_plain_package_specs() {
        assert_eq!(
            split_package("@agentclientprotocol/codex-acp@2.12.1"),
            Some(("@agentclientprotocol/codex-acp", "2.12.1"))
        );
        assert_eq!(split_package("pi-acp@0.0.35"), Some(("pi-acp", "0.0.35")));
        assert_eq!(split_package("@scope/unversioned"), None);
        assert_eq!(split_package("unversioned"), None);
        assert_eq!(split_package("trailing@"), None);
    }

    #[test]
    fn default_handle_drops_only_a_transport_suffix() {
        assert_eq!(default_handle("pi-acp"), "pi");
        assert_eq!(default_handle("gemini"), "gemini");
        assert_eq!(default_handle("antigravity-acp"), "antigravity");
        assert_eq!(default_handle("-acp"), "-acp");
    }

    #[test]
    fn cache_round_trips_and_ignores_garbage() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_cached(tmp.path()).is_none());
        std::fs::write(index_path(tmp.path()), SAMPLE).unwrap();
        assert_eq!(read_cached(tmp.path()).unwrap().len(), 5);
        std::fs::write(index_path(tmp.path()), b"<html>oops</html>").unwrap();
        assert!(read_cached(tmp.path()).is_none());
    }
}
