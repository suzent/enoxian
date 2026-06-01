/// Device and user identity for enoxian.
///
/// Architecture (see docs/plan/identity.md):
///
///   User      — optional root key; links many devices; signs device attestations.
///   Device    — one stable Ed25519 key per install, stored in ~/.enoxian/identity.toml.
///   Agent     — a named actor (human or AI) operating through a device in a circle.
///               Multiple agents can operate per device; they are pure labels, not keys.
///
/// Per-circle keypairs are DERIVED from the device key via HKDF-SHA256:
///   circle_key_bytes = HKDF(ikm=device_key_bytes, salt=b"enoxian-device-v1",
///                           info=b"circle/" || circle_id, len=32)
/// This gives a stable, deterministic peer ID per (device, circle) without
/// regenerating a fresh keypair on every join — which was the source of the
/// MLS re-add churn and epoch-rotation lockouts.

use anyhow::{bail, Context, Result};
use hkdf::Hkdf;
use libp2p::identity::Keypair;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::path::PathBuf;

// ── Paths ─────────────────────────────────────────────────────────────────────

fn identity_path() -> Result<PathBuf> {
    Ok(crate::config::enoxian_dir()?.join("identity.toml"))
}

// ── Serialised form ───────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone)]
pub struct IdentityFile {
    /// Hex-encoded raw 32-byte Ed25519 secret seed (not protobuf).
    pub device_key_hex: String,
    /// Human-readable label for this device, e.g. "suzy-macbook".
    pub device_label: String,
    /// Optional user handle, e.g. "suzy".  Displayed in presence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_handle: Option<String>,
    /// Hex-encoded user root public key — set once, when linked to a user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_pubkey_hex: Option<String>,
    /// Hex-encoded user attestation: sign(user_key, device_pubkey || label || issued_at).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_attestation_hex: Option<String>,
    /// BIP-39 mnemonic backup of the user key (stored only on the primary device).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_mnemonic: Option<String>,
}

// ── DeviceIdentity — the in-memory handle ────────────────────────────────────

#[derive(Clone)]
pub struct DeviceIdentity {
    /// Raw 32-byte Ed25519 secret seed.
    seed: [u8; 32],
    pub device_label: String,
    pub user_handle: Option<String>,
    pub user_pubkey_hex: Option<String>,
    pub user_attestation_hex: Option<String>,
}

impl DeviceIdentity {
    // ── Generation ────────────────────────────────────────────────────────────

    /// Generate a fresh device identity with the given label.
    pub fn generate(device_label: String) -> Self {
        let mut seed = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut seed);
        DeviceIdentity { seed, device_label, user_handle: None, user_pubkey_hex: None, user_attestation_hex: None }
    }

    // ── Persistence ───────────────────────────────────────────────────────────

    pub fn save(&self) -> Result<()> {
        let path = identity_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = IdentityFile {
            device_key_hex: hex::encode(self.seed),
            device_label: self.device_label.clone(),
            user_handle: self.user_handle.clone(),
            user_pubkey_hex: self.user_pubkey_hex.clone(),
            user_attestation_hex: self.user_attestation_hex.clone(),
            user_mnemonic: None, // never write mnemonic back
        };
        let toml = toml::to_string_pretty(&file).context("serialize identity")?;
        std::fs::write(&path, toml)?;
        Ok(())
    }

    pub fn load() -> Result<Self> {
        let path = identity_path()?;
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("read {}", path.display()))?;
        let file: IdentityFile = toml::from_str(&raw).context("parse identity.toml")?;
        let bytes = hex::decode(&file.device_key_hex).context("decode device_key_hex")?;
        if bytes.len() != 32 {
            bail!("device_key_hex must be 32 bytes");
        }
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&bytes);
        Ok(DeviceIdentity {
            seed,
            device_label: file.device_label,
            user_handle: file.user_handle,
            user_pubkey_hex: file.user_pubkey_hex,
            user_attestation_hex: file.user_attestation_hex,
        })
    }

    pub fn load_or_generate(device_label: Option<String>) -> Result<Self> {
        let path = identity_path()?;
        if path.exists() {
            return Self::load();
        }
        let label = device_label.unwrap_or_else(default_device_label);
        let id = Self::generate(label);
        id.save()?;
        Ok(id)
    }

    pub fn exists() -> bool {
        identity_path().map(|p| p.exists()).unwrap_or(false)
    }

    // ── Key derivation ────────────────────────────────────────────────────────

    /// Derive a stable Ed25519 keypair for a specific circle via HKDF-SHA256.
    /// The same device + circle_id always produces the same keypair, so the
    /// libp2p peer ID is stable across daemon restarts and re-joins.
    pub fn derive_circle_keypair(&self, circle_id: &str) -> Result<Keypair> {
        let hk = Hkdf::<Sha256>::new(Some(b"enoxian-device-v1"), &self.seed);
        let info = format!("circle/{circle_id}");
        let mut okm = [0u8; 32];
        hk.expand(info.as_bytes(), &mut okm)
            .map_err(|_| anyhow::anyhow!("HKDF expand failed"))?;
        // libp2p's ed25519::SecretKey::try_from_bytes takes exactly 32 raw bytes.
        let secret = libp2p::identity::ed25519::SecretKey::try_from_bytes(okm)
            .map_err(|e| anyhow::anyhow!("ed25519 secret: {e}"))?;
        let kp = libp2p::identity::ed25519::Keypair::from(secret);
        Ok(Keypair::from(kp))
    }

    /// The stable device keypair (not circle-specific).  Used for user
    /// attestation signing and as the identity root.
    pub fn device_keypair(&self) -> Result<Keypair> {
        self.derive_circle_keypair("__device__")
    }

    // ── Display helpers ───────────────────────────────────────────────────────

    /// The name shown in presence: user_handle if set, otherwise device_label.
    pub fn display_name(&self) -> &str {
        self.user_handle.as_deref().unwrap_or(&self.device_label)
    }

    // ── User linking ──────────────────────────────────────────────────────────

    pub fn set_user_handle(&mut self, handle: String) {
        self.user_handle = Some(handle);
    }
}

// ── User identity (root key that links devices) ───────────────────────────────

pub struct UserIdentity {
    seed: [u8; 32],
    pub handle: String,
}

impl UserIdentity {
    /// Generate a brand-new user identity.  Returns the identity and the
    /// 24-word BIP-39 mnemonic the user must back up.
    pub fn generate(handle: String) -> Result<(Self, String)> {
        let mut seed = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut seed);
        let mnemonic = bip39::Mnemonic::from_entropy(&seed)
            .context("generate mnemonic")?
            .to_string();
        Ok((UserIdentity { seed, handle }, mnemonic))
    }

    /// Restore from a BIP-39 mnemonic (used when linking a second device).
    pub fn from_mnemonic(mnemonic: &str, handle: String) -> Result<Self> {
        let m = mnemonic.parse::<bip39::Mnemonic>().context("parse mnemonic")?;
        let seed_bytes = m.to_entropy();
        if seed_bytes.len() != 32 {
            bail!("mnemonic entropy must be 32 bytes");
        }
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&seed_bytes);
        Ok(UserIdentity { seed, handle })
    }

    fn keypair(&self) -> Result<Keypair> {
        let hk = Hkdf::<Sha256>::new(Some(b"enoxian-user-v1"), &self.seed);
        let mut okm = [0u8; 32];
        hk.expand(b"user-root-key", &mut okm).map_err(|_| anyhow::anyhow!("HKDF failed"))?;
        let secret = libp2p::identity::ed25519::SecretKey::try_from_bytes(okm)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(Keypair::from(libp2p::identity::ed25519::Keypair::from(secret)))
    }

    pub fn pubkey_hex(&self) -> Result<String> {
        Ok(hex::encode(self.keypair()?.public().encode_protobuf()))
    }

    /// Sign an attestation binding a device pubkey to this user.
    /// Returns hex-encoded signature over (device_pubkey_bytes || device_label).
    pub fn attest_device(&self, device_pubkey_hex: &str, device_label: &str) -> Result<String> {
        let kp = self.keypair()?;
        let device_bytes = hex::decode(device_pubkey_hex).context("decode device pubkey")?;
        let mut msg = device_bytes;
        msg.extend_from_slice(device_label.as_bytes());
        let ed = kp.try_into_ed25519().map_err(|e| anyhow::anyhow!("{e}"))?;
        let sig = ed.sign(&msg);
        Ok(hex::encode(sig))
    }

    /// Link this user to a device identity (mutates device; saves both).
    pub fn link_device(&self, device: &mut DeviceIdentity, mnemonic: &str) -> Result<()> {
        let device_pubkey = hex::encode(device.device_keypair()?.public().encode_protobuf());
        let attestation = self.attest_device(&device_pubkey, &device.device_label)?;
        device.user_handle = Some(self.handle.clone());
        device.user_pubkey_hex = Some(self.pubkey_hex()?);
        device.user_attestation_hex = Some(attestation);
        // Store mnemonic on the primary device (where UserIdentity was generated).
        let path = identity_path()?;
        if path.exists() {
            let raw = std::fs::read_to_string(&path)?;
            let mut file: IdentityFile = toml::from_str(&raw)?;
            file.user_handle = device.user_handle.clone();
            file.user_pubkey_hex = device.user_pubkey_hex.clone();
            file.user_attestation_hex = device.user_attestation_hex.clone();
            file.user_mnemonic = Some(mnemonic.to_string());
            std::fs::write(&path, toml::to_string_pretty(&file)?)?;
        } else {
            device.save()?;
        }
        Ok(())
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn default_device_label() -> String {
    // hostname stripped of domain suffix, falls back to "device"
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().split('.').next().unwrap_or("device").to_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "device".to_string())
}

/// Read just the identity file without constructing a full DeviceIdentity.
/// Used by the status/CLI to display identity info without key material.
pub fn read_identity_display() -> Option<(String, Option<String>)> {
    identity_path().ok().and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| toml::from_str::<IdentityFile>(&s).ok())
        .map(|f| (f.device_label, f.user_handle))
}

/// These tests access private fields (`seed`, `IdentityFile`) and must live
/// inline. All tests using only the public API live in `tests/identity.rs`.
#[cfg(test)]
mod tests {
    use super::*;

    // TOML round-trip — needs `IdentityFile` (private) and `seed` (private).
    #[test]
    fn identity_file_toml_round_trip() {
        let d = DeviceIdentity::generate("my-machine".to_string());
        let mut d_with_user = DeviceIdentity::generate("my-machine".to_string());
        d_with_user.user_handle = Some("alice".to_string());

        for device in [&d, &d_with_user] {
            let file = IdentityFile {
                device_key_hex: hex::encode(device.seed),
                device_label: device.device_label.clone(),
                user_handle: device.user_handle.clone(),
                user_pubkey_hex: None,
                user_attestation_hex: None,
                user_mnemonic: None,
            };
            let toml_str = toml::to_string_pretty(&file).unwrap();
            let loaded: IdentityFile = toml::from_str(&toml_str).unwrap();
            assert_eq!(loaded.device_key_hex, hex::encode(device.seed));
            assert_eq!(loaded.device_label, device.device_label);
            assert_eq!(loaded.user_handle, device.user_handle);
        }
    }

    // Seed stability — reconstructing DeviceIdentity from the same seed bytes
    // (as load() does) must produce the same peer ID. Needs private seed field.
    #[test]
    fn same_seed_gives_same_peer_id() {
        let d = DeviceIdentity::generate("stable-test".to_string());
        let pid_before = d.derive_circle_keypair("c1").unwrap().public().to_peer_id();

        let d2 = DeviceIdentity {
            seed: d.seed,
            device_label: d.device_label.clone(),
            user_handle: None,
            user_pubkey_hex: None,
            user_attestation_hex: None,
        };
        assert_eq!(pid_before, d2.derive_circle_keypair("c1").unwrap().public().to_peer_id());
    }
}
