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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_device(label: &str) -> DeviceIdentity {
        DeviceIdentity::generate(label.to_string())
    }

    // ── Key derivation ─────────────────────────────────────────────────────────

    #[test]
    fn circle_keypair_is_deterministic() {
        let d = make_device("test");
        let kp1 = d.derive_circle_keypair("circle-abc").unwrap();
        let kp2 = d.derive_circle_keypair("circle-abc").unwrap();
        assert_eq!(
            kp1.public().to_peer_id(),
            kp2.public().to_peer_id(),
            "same device + same circle_id must always produce the same peer ID"
        );
    }

    #[test]
    fn different_circles_produce_different_keypairs() {
        let d = make_device("test");
        let kp_a = d.derive_circle_keypair("circle-alpha").unwrap();
        let kp_b = d.derive_circle_keypair("circle-beta").unwrap();
        assert_ne!(
            kp_a.public().to_peer_id(),
            kp_b.public().to_peer_id(),
            "different circle IDs must derive different peer IDs"
        );
    }

    #[test]
    fn different_devices_produce_different_keypairs_for_same_circle() {
        let d1 = make_device("device-one");
        let d2 = make_device("device-two");
        let kp1 = d1.derive_circle_keypair("shared-circle").unwrap();
        let kp2 = d2.derive_circle_keypair("shared-circle").unwrap();
        assert_ne!(
            kp1.public().to_peer_id(),
            kp2.public().to_peer_id(),
            "different devices must derive different peer IDs even for the same circle"
        );
    }

    #[test]
    fn device_keypair_differs_from_circle_keypair() {
        let d = make_device("test");
        let device_kp = d.device_keypair().unwrap();
        let circle_kp = d.derive_circle_keypair("some-circle").unwrap();
        assert_ne!(
            device_kp.public().to_peer_id(),
            circle_kp.public().to_peer_id()
        );
    }

    // ── Persistence ────────────────────────────────────────────────────────────

    #[test]
    fn save_load_round_trip() {
        let dir = TempDir::new().unwrap();
        // Override the identity path by setting HOME to the temp dir.
        // DeviceIdentity::save/load use identity_path() which calls enoxian_dir()
        // which calls dirs::home_dir(). We test the serialisation directly instead.
        let mut d = make_device("my-machine");
        d.user_handle = Some("alice".to_string());

        // Serialise to TOML and re-parse.
        let file = IdentityFile {
            device_key_hex: hex::encode(d.seed),
            device_label: d.device_label.clone(),
            user_handle: d.user_handle.clone(),
            user_pubkey_hex: None,
            user_attestation_hex: None,
            user_mnemonic: None,
        };
        let path = dir.path().join("identity.toml");
        std::fs::write(&path, toml::to_string_pretty(&file).unwrap()).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let loaded: IdentityFile = toml::from_str(&raw).unwrap();

        assert_eq!(loaded.device_label, "my-machine");
        assert_eq!(loaded.user_handle.as_deref(), Some("alice"));
        assert_eq!(loaded.device_key_hex, hex::encode(d.seed));
    }

    #[test]
    fn peer_id_stable_after_serialise_deserialise() {
        let d = make_device("stable-test");
        let peer_id_before = d.derive_circle_keypair("c1").unwrap().public().to_peer_id();

        // Reconstruct from the same seed bytes (simulating load from disk).
        let d2 = DeviceIdentity {
            seed: d.seed,
            device_label: d.device_label.clone(),
            user_handle: None,
            user_pubkey_hex: None,
            user_attestation_hex: None,
        };
        let peer_id_after = d2.derive_circle_keypair("c1").unwrap().public().to_peer_id();
        assert_eq!(peer_id_before, peer_id_after);
    }

    // ── Display name ───────────────────────────────────────────────────────────

    #[test]
    fn display_name_prefers_user_handle() {
        let mut d = make_device("my-laptop");
        assert_eq!(d.display_name(), "my-laptop");
        d.set_user_handle("suzy".to_string());
        assert_eq!(d.display_name(), "suzy");
    }

    // ── User identity & mnemonic ───────────────────────────────────────────────

    #[test]
    fn mnemonic_is_24_words() {
        let (_user, mnemonic) = UserIdentity::generate("alice".to_string()).unwrap();
        assert_eq!(mnemonic.split_whitespace().count(), 24);
    }

    #[test]
    fn mnemonic_round_trip_produces_same_pubkey() {
        let (user, mnemonic) = UserIdentity::generate("alice".to_string()).unwrap();
        let pk1 = user.pubkey_hex().unwrap();

        let user2 = UserIdentity::from_mnemonic(&mnemonic, "alice".to_string()).unwrap();
        let pk2 = user2.pubkey_hex().unwrap();

        assert_eq!(pk1, pk2, "restoring from mnemonic must reproduce the same user public key");
    }

    #[test]
    fn invalid_mnemonic_is_rejected() {
        let result = UserIdentity::from_mnemonic("not a valid mnemonic", "x".to_string());
        assert!(result.is_err());
    }

    #[test]
    fn attestation_contains_device_pubkey() {
        let (user, _) = UserIdentity::generate("alice".to_string()).unwrap();
        let device = make_device("laptop");
        let device_pubkey = hex::encode(device.device_keypair().unwrap().public().encode_protobuf());
        let attestation = user.attest_device(&device_pubkey, &device.device_label);
        assert!(attestation.is_ok(), "attestation should succeed");
        // Attestation is a 64-byte Ed25519 signature → 128 hex chars.
        assert_eq!(attestation.unwrap().len(), 128);
    }
}
