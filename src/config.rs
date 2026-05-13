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
}

pub fn circles_dir() -> Result<PathBuf> {
    let base = dirs::home_dir().context("cannot resolve home directory")?;
    Ok(base.join(".enochian").join("circles"))
}

pub fn circle_dir(circle_id: &str) -> Result<PathBuf> {
    Ok(circles_dir()?.join(circle_id))
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
    toml::from_str(&contents).context("failed to parse config.toml")
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
                Ok(cfg) => configs.push(cfg),
                Err(e) => tracing::warn!("skipping {}: {e}", config_path.display()),
            }
        }
    }
    configs.sort_by(|a: &CircleConfig, b: &CircleConfig| a.circle_name.cmp(&b.circle_name));
    Ok(configs)
}
