use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum JoinPolicy {
    #[default]
    Auto,
    Manual,
}

impl std::fmt::Display for JoinPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auto => write!(f, "auto"),
            Self::Manual => write!(f, "manual"),
        }
    }
}

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
    /// If true, the daemon skips this circle at startup and does not start its swarm.
    #[serde(default)]
    pub disabled: bool,
    /// Diagnostic mode: only connect to circle peers through circuit relay.
    #[serde(default)]
    pub force_relay: bool,
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
    #[serde(default)]
    pub join_policy: JoinPolicy,
    #[serde(default)]
    pub owner: String,
}

pub fn enoxian_dir() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("ENOXIAN_HOME") {
        return Ok(PathBuf::from(path));
    }
    let base = dirs::home_dir().context("cannot resolve home directory")?;
    let current = base.join(".enoxian");
    let legacy = base.join(".enochian");
    if !current.exists() && legacy.exists() {
        if std::fs::rename(&legacy, &current).is_ok() || current.exists() {
            return Ok(current);
        }
        return Ok(legacy);
    }
    Ok(current)
}

pub fn circles_dir() -> Result<PathBuf> {
    Ok(enoxian_dir()?.join("circles"))
}

// ── Global config ──────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct GlobalConfig {
    /// Path to the enoxian source directory, saved by `enox update --dev`.
    #[serde(default)]
    pub dev_src: Option<String>,
    /// Active update source (`stable` or `dev`).
    #[serde(default)]
    pub update_channel: Option<String>,
    /// Binary path owned by the installed login service.
    #[serde(default)]
    pub managed_executable: Option<String>,
}

pub fn global_config_path() -> Result<PathBuf> {
    Ok(enoxian_dir()?.join("config.toml"))
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
    std::fs::write(
        path,
        toml::to_string_pretty(cfg).context("serialize failed")?,
    )?;
    Ok(())
}

pub fn circle_dir(circle_id: &str) -> Result<PathBuf> {
    Ok(circles_dir()?.join(circle_id))
}

/// Default workspace root: ~/enoxian/
pub fn workspace_root() -> Result<PathBuf> {
    let base = dirs::home_dir().context("cannot resolve home directory")?;
    Ok(base.join("enoxian"))
}

/// Default workspace for a circle: ~/enoxian/<circle-name>/
pub fn default_workspace_dir(circle_name: &str) -> Result<PathBuf> {
    Ok(workspace_root()?.join(circle_name))
}

/// Workspace dir for a circle that would collide on name: ~/enoxian/<name>-<id[..6]>/
pub fn disambiguated_workspace_dir(circle_name: &str, circle_id: &str) -> Result<PathBuf> {
    let short_id = &circle_id.replace('-', "")[..6];
    Ok(workspace_root()?.join(format!("{circle_name}-{short_id}")))
}

/// Return a stable absolute representation for workspace ownership checks.
/// Lexical aliases are collapsed before checking existence. This matters on
/// macOS, where a temp path may be exposed as `/var/...` but canonicalize to
/// `/private/var/...`: `workspace/missing/..` does not itself exist, while its
/// collapsed target does and must receive the same canonical representation.
pub fn normalize_workspace_dir(path: &std::path::Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    anyhow::bail!(
                        "workspace path escapes its filesystem root: {}",
                        path.display()
                    );
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    if normalized.exists() {
        std::fs::canonicalize(&normalized)
            .with_context(|| format!("failed to resolve workspace {}", normalized.display()))
    } else {
        Ok(normalized)
    }
}

fn workspace_key(path: &std::path::Path) -> Result<String> {
    let normalized = normalize_workspace_dir(path)?;
    let key = normalized.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    let key = key.to_lowercase();
    Ok(key)
}

pub fn workspace_paths_equal(a: &std::path::Path, b: &std::path::Path) -> Result<bool> {
    Ok(workspace_key(a)? == workspace_key(b)?)
}

/// Find another configured Circle that already owns this workspace path.
pub fn workspace_conflict<'a>(
    workspace: &std::path::Path,
    circle_id: &str,
    existing: &'a [CircleConfig],
) -> Result<Option<&'a CircleConfig>> {
    let wanted = workspace_key(workspace)?;
    for config in existing {
        if config.circle_id == circle_id {
            continue;
        }
        let configured = if config.workspace_dir.is_empty() {
            default_workspace_dir(&config.circle_name)?
        } else {
            PathBuf::from(&config.workspace_dir)
        };
        if workspace_key(&configured)? == wanted {
            return Ok(Some(config));
        }
    }
    Ok(None)
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
        let dir = normalize_workspace_dir(&dir)?;
        if let Some(conflict) = workspace_conflict(&dir, circle_id, existing)? {
            anyhow::bail!(
                "workspace {} is already owned by circle '{}' ({})",
                dir.display(),
                conflict.circle_name,
                conflict.circle_id
            );
        }
        return Ok(Some((dir, None)));
    }

    let default = default_workspace_dir(circle_name)?;

    // Check if the default workspace is already claimed by a different circle
    let name_clash = existing
        .iter()
        .any(|c| c.circle_name == circle_name && c.circle_id != circle_id);

    let path_clash = workspace_conflict(&default, circle_id, existing)?.is_some();

    if name_clash || path_clash {
        let dir = normalize_workspace_dir(&disambiguated_workspace_dir(circle_name, circle_id)?)?;
        if let Some(conflict) = workspace_conflict(&dir, circle_id, existing)? {
            anyhow::bail!(
                "workspace {} is already owned by circle '{}' ({})",
                dir.display(),
                conflict.circle_name,
                conflict.circle_id
            );
        }
        let warn = format!(
            "⚠ A circle named '{}' already exists locally.\n  Workspace → {}",
            circle_name,
            dir.display()
        );
        Ok(Some((dir, Some(warn))))
    } else {
        Ok(Some((normalize_workspace_dir(&default)?, None)))
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
    let mut config: CircleConfig =
        toml::from_str(&contents).context("failed to parse config.toml")?;
    // Migrate: fill in workspace_dir if missing from older configs
    if config.workspace_dir.is_empty() {
        config.workspace_dir = default_workspace_dir(&config.circle_name)?
            .to_string_lossy()
            .into_owned();
    }
    Ok(config)
}

/// Load every circle config found under ~/.enoxian/circles/*/config.toml.
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
            match std::fs::read_to_string(&config_path).and_then(|s| {
                toml::from_str(&s)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
            }) {
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

#[cfg(test)]
mod workspace_tests {
    use super::*;

    fn circle(id: &str, name: &str, workspace: &std::path::Path) -> CircleConfig {
        CircleConfig {
            circle_id: id.into(),
            circle_name: name.into(),
            psk_hex: String::new(),
            keypair_proto_hex: String::new(),
            workspace_dir: workspace.to_string_lossy().into_owned(),
            admin_pubkey_hex: String::new(),
            disabled: false,
            force_relay: false,
            peers: vec![],
            relay_addrs: vec![],
            rendezvous_addrs: vec![],
            join_policy: JoinPolicy::Auto,
            owner: String::new(),
        }
    }

    #[test]
    fn old_config_defaults_force_relay_to_false() {
        let raw = r#"
circle_id = "old-circle"
circle_name = "Old"
psk_hex = ""
keypair_proto_hex = ""
"#;
        let config: CircleConfig = toml::from_str(raw).unwrap();
        assert!(!config.force_relay);
    }

    #[test]
    fn detects_workspace_owned_by_another_circle() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("shared");
        std::fs::create_dir_all(&workspace).unwrap();
        let existing = vec![circle("circle-a", "A", &workspace)];

        let conflict = workspace_conflict(&workspace, "circle-b", &existing)
            .unwrap()
            .expect("same workspace must conflict");
        assert_eq!(conflict.circle_id, "circle-a");
        assert!(workspace_conflict(&workspace, "circle-a", &existing)
            .unwrap()
            .is_none());
    }

    #[test]
    fn relative_aliases_normalize_to_the_same_workspace() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let alias = workspace.join("child").join("..");
        assert!(workspace_paths_equal(&workspace, &alias).unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn workspace_comparison_is_case_insensitive_on_windows() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("CaseSensitiveLookingName");
        assert!(workspace_paths_equal(
            &workspace,
            &PathBuf::from(workspace.to_string_lossy().to_lowercase())
        )
        .unwrap());
    }
}
