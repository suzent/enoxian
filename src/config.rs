use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CircleConfig {
    pub circle_id: String,
    pub circle_name: String,
    /// 32-byte PSK encoded as 64-char hex
    pub psk_hex: String,
    /// Ed25519 keypair encoded via protobuf (libp2p canonical format)
    pub keypair_proto_hex: String,
    /// Absolute path to the workspace directory for this circle
    #[serde(default)]
    pub workspace_dir: String,
    /// Admin public key hex (Ed25519). Present on all peers; private key only on admin machines.
    #[serde(default)]
    pub admin_pubkey_hex: String,
    /// If true, enochd skips this circle at startup and does not start its swarm.
    #[serde(default)]
    pub disabled: bool,
    /// Known peer multiaddrs (e.g. from invite). Dialed on startup as bootstrap
    /// peers in addition to mDNS discovery.
    #[serde(default)]
    pub peers: Vec<String>,
    /// Circuit relay multiaddrs. On startup we connect to each relay and listen
    /// on a p2p-circuit address so peers behind NAT can reach us.
    #[serde(default)]
    pub relay_addrs: Vec<String>,
    /// Rendezvous server multiaddrs (QUIC). On startup we dial these, register
    /// our circle namespace, and discover other members via rendezvous.
    #[serde(default)]
    pub rendezvous_addrs: Vec<String>,
}

pub fn enochian_dir() -> Result<PathBuf> {
    let base = dirs::home_dir().context("cannot resolve home directory")?;
    Ok(base.join(".enochian"))
}

pub fn circles_dir() -> Result<PathBuf> {
    Ok(enochian_dir()?.join("circles"))
}

// ── Global config ──────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct GlobalConfig {
    /// Path to the enochian source directory, saved by `enoch update --dev`.
    #[serde(default)]
    pub dev_src: Option<String>,
}

pub fn global_config_path() -> Result<PathBuf> {
    Ok(enochian_dir()?.join("config.toml"))
}

pub fn load_global() -> GlobalConfig {
    global_config_path()
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_global(cfg: &GlobalConfig) -> Result<()> {
    let path = global_config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, toml::to_string_pretty(cfg).context("serialize failed")?)?;
    Ok(())
}

pub fn circle_dir(circle_id: &str) -> Result<PathBuf> {
    Ok(circles_dir()?.join(circle_id))
}

/// Default workspace root: ~/enochian/
pub fn workspace_root() -> Result<PathBuf> {
    let base = dirs::home_dir().context("cannot resolve home directory")?;
    Ok(base.join("enochian"))
}

/// Default workspace for a circle: ~/enochian/<circle-name>/
pub fn default_workspace_dir(circle_name: &str) -> Result<PathBuf> {
    Ok(workspace_root()?.join(circle_name))
}

/// Workspace dir for a circle that would collide on name: ~/enochian/<name>-<id[..6]>/
pub fn disambiguated_workspace_dir(circle_name: &str, circle_id: &str) -> Result<PathBuf> {
    let short_id = &circle_id.replace('-', "")[..6];
    Ok(workspace_root()?.join(format!("{circle_name}-{short_id}")))
}

/// Resolve the workspace dir for a circle being joined, handling name conflicts.
/// - Same UUID already exists → returns None (caller should skip)
/// - Same name, different UUID → uses disambiguated path and returns a warning string
/// - No conflict → uses default path
pub fn resolve_workspace_dir(
    circle_name: &str,
    circle_id: &str,
    existing: &[CircleConfig],
    override_dir: Option<PathBuf>,
) -> Result<Option<(PathBuf, Option<String>)>> {
    // Exact UUID match — already a member
    if existing.iter().any(|c| c.circle_id == circle_id) {
        return Ok(None);
    }

    if let Some(dir) = override_dir {
        return Ok(Some((dir, None)));
    }

    let default = default_workspace_dir(circle_name)?;

    // Check if the default workspace is already claimed by a different circle
    let name_clash = existing.iter().any(|c| {
        c.circle_name == circle_name && c.circle_id != circle_id
    });

    if name_clash {
        let dir = disambiguated_workspace_dir(circle_name, circle_id)?;
        let warn = format!(
            "⚠ A circle named '{}' already exists locally.\n  Workspace → {}",
            circle_name,
            dir.display()
        );
        Ok(Some((dir, Some(warn))))
    } else {
        Ok(Some((default, None)))
    }
}

pub fn save(config: &CircleConfig) -> Result<()> {
    let dir = circle_dir(&config.circle_id)?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create circle dir {}", dir.display()))?;
    let path = dir.join("config.toml");
    let contents = toml::to_string_pretty(config).context("failed to serialize config")?;
    std::fs::write(&path, contents)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

pub fn load(circle_id: &str) -> Result<CircleConfig> {
    let path = circle_dir(circle_id)?.join("config.toml");
    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let mut config: CircleConfig = toml::from_str(&contents).context("failed to parse config.toml")?;
    // Migrate: fill in workspace_dir if missing from older configs
    if config.workspace_dir.is_empty() {
        config.workspace_dir = default_workspace_dir(&config.circle_name)?
            .to_string_lossy()
            .into_owned();
    }
    Ok(config)
}

/// Load every circle config found under ~/.enochian/circles/*/config.toml.
pub fn load_all() -> Result<Vec<CircleConfig>> {
    let dir = circles_dir()?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut configs = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .with_context(|| format!("failed to read circles dir {}", dir.display()))?
    {
        let entry = entry?;
        let config_path = entry.path().join("config.toml");
        if config_path.exists() {
            match std::fs::read_to_string(&config_path)
                .and_then(|s| toml::from_str(&s).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e)))
            {
                Ok(cfg) => {
                    let cfg: CircleConfig = cfg;
                    // Migrate workspace_dir
                    if cfg.workspace_dir.is_empty() {
                        let mut cfg = cfg;
                        cfg.workspace_dir = default_workspace_dir(&cfg.circle_name)?
                            .to_string_lossy()
                            .into_owned();
                        configs.push(cfg);
                    } else {
                        configs.push(cfg);
                    }
                }
                Err(e) => tracing::warn!("skipping {}: {e}", config_path.display()),
            }
        }
    }
    configs.sort_by(|a, b| a.circle_name.cmp(&b.circle_name));
    Ok(configs)
}
