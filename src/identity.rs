/// Device and user identity for enoxian.
///
/// Architecture (see docs/concepts/security.md):
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
use rand::Rng;
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
    /// Agent names registered on this device (e.g. ["human", "claude-code"]).
    /// These are pure labels — all share the device key. Enrolled into circles at join time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<String>,
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
    pub agents: Vec<String>,
}

impl DeviceIdentity {
    // ── Generation ────────────────────────────────────────────────────────────

    /// Generate a fresh device identity with the given label.
    pub fn generate(device_label: String) -> Self {
        let mut seed = [0u8; 32];
        rand::rng().fill_bytes(&mut seed);
        DeviceIdentity {
            seed,
            device_label,
            user_handle: None,
            user_pubkey_hex: None,
            user_attestation_hex: None,
            agents: Vec::new(),
        }
    }

    // ── Persistence ───────────────────────────────────────────────────────────

    pub fn save(&self) -> Result<()> {
        let path = identity_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Carry the recovery phrase forward. It lives only in the file —
        // `DeviceIdentity` has nowhere to hold it — so rewriting without
        // reading first destroys the single copy of the user root key. This
        // used to happen on every `enox identity set-label` and on receiving a
        // link, silently and with no way back.
        let user_mnemonic = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| toml::from_str::<IdentityFile>(&raw).ok())
            .and_then(|existing| existing.user_mnemonic);

        let file = IdentityFile {
            device_key_hex: hex::encode(self.seed),
            device_label: self.device_label.clone(),
            user_handle: self.user_handle.clone(),
            user_pubkey_hex: self.user_pubkey_hex.clone(),
            user_attestation_hex: self.user_attestation_hex.clone(),
            user_mnemonic,
            agents: self.agents.clone(),
        };
        let toml = toml::to_string_pretty(&file).context("serialize identity")?;
        // Holds the device seed: anyone who can read it can be this device.
        crate::config::write_secret(&path, toml)?;
        Ok(())
    }

    pub fn load() -> Result<Self> {
        let path = identity_path()?;
        // Installs made before secrets were written with a restrictive mode
        // still have a world-readable device seed sitting there; this is the
        // one moment we are certain to be looking at the file.
        crate::config::tighten_if_loose(&path);
        let raw =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
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
            agents: file.agents,
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

    /// The user identity this device holds the root key for, if any.
    ///
    /// Only the device the user identity was created on stores the mnemonic, so
    /// this is `None` on a device that was itself linked. That device can still
    /// pass on its circles; it just cannot sign an attestation for a third one.
    pub fn user_identity(&self) -> Result<Option<UserIdentity>> {
        let path = identity_path()?;
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Ok(None);
        };
        let file: IdentityFile = toml::from_str(&raw).context("parse identity.toml")?;
        let (Some(mnemonic), Some(handle)) = (file.user_mnemonic, file.user_handle) else {
            return Ok(None);
        };
        Ok(Some(UserIdentity::from_mnemonic(&mnemonic, handle)?))
    }

    /// Adopt a user identity that another device attested for this one.
    ///
    /// Deliberately takes the attestation rather than the root key: the device
    /// that signed keeps the only copy, and this device gets an identity of its
    /// own that can be revoked without touching the others.
    ///
    /// The signature is checked here, against this device's own key, so a
    /// device cannot end up storing an identity it has no proof of. Storing one
    /// unchecked would be invisible until something tried to rely on it.
    pub fn adopt_attestation(
        &mut self,
        handle: String,
        user_pubkey_hex: String,
        attestation_hex: String,
    ) -> Result<()> {
        let device_pubkey_hex = hex::encode(self.device_keypair()?.public().encode_protobuf());
        if !UserIdentity::verify_attestation(
            &user_pubkey_hex,
            &device_pubkey_hex,
            &attestation_hex,
        )? {
            bail!(
                "the attestation from the other device does not cover this device's key — \
                 nothing was saved"
            );
        }
        self.user_handle = Some(handle);
        self.user_pubkey_hex = Some(user_pubkey_hex);
        self.user_attestation_hex = Some(attestation_hex);
        Ok(())
    }

    /// The user identity this device already claims, if any.
    pub fn claimed_user_pubkey(&self) -> Option<&str> {
        self.user_pubkey_hex.as_deref()
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
        rand::rng().fill_bytes(&mut seed);
        let mnemonic = bip39::Mnemonic::from_entropy(&seed)
            .context("generate mnemonic")?
            .to_string();
        Ok((UserIdentity { seed, handle }, mnemonic))
    }

    /// Restore from a BIP-39 mnemonic (used when linking a second device).
    pub fn from_mnemonic(mnemonic: &str, handle: String) -> Result<Self> {
        let m = mnemonic
            .parse::<bip39::Mnemonic>()
            .context("parse mnemonic")?;
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
        hk.expand(b"user-root-key", &mut okm)
            .map_err(|_| anyhow::anyhow!("HKDF failed"))?;
        let secret = libp2p::identity::ed25519::SecretKey::try_from_bytes(okm)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(Keypair::from(libp2p::identity::ed25519::Keypair::from(
            secret,
        )))
    }

    pub fn pubkey_hex(&self) -> Result<String> {
        Ok(hex::encode(self.keypair()?.public().encode_protobuf()))
    }

    /// Sign an attestation binding a device public key to this user.
    pub fn attest_device(&self, device_pubkey_hex: &str) -> Result<String> {
        let msg = attestation_message(device_pubkey_hex)?;
        let ed = self
            .keypair()?
            .try_into_ed25519()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(hex::encode(ed.sign(&msg)))
    }

    /// Check an attestation against the user key that supposedly signed it.
    ///
    /// Nothing in the membership layer calls this yet — `MemberEntry.owner` is
    /// still a self-asserted string, and binding it to a verified attestation is
    /// its own change. It exists now because `enox link` ships an attestation to
    /// another device, and an attestation nobody can check is not worth sending.
    pub fn verify_attestation(
        user_pubkey_hex: &str,
        device_pubkey_hex: &str,
        attestation_hex: &str,
    ) -> Result<bool> {
        let user_bytes = hex::decode(user_pubkey_hex.trim()).context("decode user pubkey")?;
        let user_key = libp2p::identity::PublicKey::try_decode_protobuf(&user_bytes)
            .map_err(|e| anyhow::anyhow!("invalid user public key: {e}"))?;
        let sig = hex::decode(attestation_hex.trim()).context("decode attestation")?;
        Ok(user_key.verify(&attestation_message(device_pubkey_hex)?, &sig))
    }

    /// Link this user to a device identity (mutates device; saves both).
    pub fn link_device(&self, device: &mut DeviceIdentity, mnemonic: &str) -> Result<()> {
        let device_pubkey = hex::encode(device.device_keypair()?.public().encode_protobuf());
        let attestation = self.attest_device(&device_pubkey)?;
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

/// Domain tag on an attestation, so a signature made here can never be read as
/// one made by some other part of the system with the same key.
const ATTESTATION_DOMAIN: &[u8] = b"enoxian-device-attestation-v1";

/// The exact bytes an attestation covers.
///
/// Length-prefixed, and over the device key alone. The previous encoding was
/// `device_pubkey_bytes || device_label` with no prefix, which is ambiguous:
/// an attestation for (key `AABB`, label `CC`) covers the identical bytes as
/// one for (key `AABBCC`, label ``), so a single signature could be claimed by
/// two different device identities. Nothing validated the key either, so the
/// "key" could be any length the claimant liked.
///
/// The label is deliberately not signed. It is a display name the user can
/// change at will, and binding it here meant `enox identity set-label` silently
/// and permanently invalidated the device's attestation.
fn attestation_message(device_pubkey_hex: &str) -> Result<Vec<u8>> {
    let device_bytes = hex::decode(device_pubkey_hex.trim()).context("decode device pubkey")?;
    // A device key is a libp2p public key. Refusing anything else keeps the
    // root key from being talked into signing bytes of somebody's choosing.
    libp2p::identity::PublicKey::try_decode_protobuf(&device_bytes)
        .map_err(|e| anyhow::anyhow!("device public key is not a valid key: {e}"))?;

    let len = u32::try_from(device_bytes.len()).context("device pubkey is implausibly long")?;
    let mut msg = Vec::with_capacity(ATTESTATION_DOMAIN.len() + 4 + device_bytes.len());
    msg.extend_from_slice(ATTESTATION_DOMAIN);
    msg.extend_from_slice(&len.to_be_bytes());
    msg.extend_from_slice(&device_bytes);
    Ok(msg)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn default_device_label() -> String {
    // hostname stripped of domain suffix, falls back to "device"
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| {
            s.trim()
                .split('.')
                .next()
                .unwrap_or("device")
                .to_lowercase()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "device".to_string())
}

/// Read just the identity file without constructing a full DeviceIdentity.
/// Used by the status/CLI to display identity info without key material.
pub fn read_identity_display() -> Option<(String, Option<String>)> {
    identity_path()
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| toml::from_str::<IdentityFile>(&s).ok())
        .map(|f| (f.device_label, f.user_handle))
}

/// The agents this device advertises to the circle: the explicit list in
/// `identity.toml` merged with the agents configured in `agents.toml` (the
/// allowlist of what a mention can actually launch here). Merging means a
/// device automatically advertises what it can run, without duplicating the
/// list by hand. Deduplicated, order-stable (identity first, then config).
///
/// Configured-but-unusable agents are left out, because the mention popup marks
/// an advertised agent runnable and a run that cannot start fails only after
/// someone addresses it. Two things make an entry unusable:
///
/// - its own `command[0]` does not resolve here. An agent that speaks ACP
///   itself (`suzent acp`) names the product CLI directly rather than an
///   adapter, so this is the only check that can catch it missing.
/// - it is a bridge adapter whose product CLI is absent: the adapter is only as
///   installed as the CLI underneath it.
///
/// A `runtime_download` command (`npx …`) stays advertised. Device Settings
/// deliberately treats it as not-ready to nudge migration, but it can still
/// start, and dropping it here would silently unadvertise working legacy
/// configs.
pub fn read_local_agents() -> Vec<String> {
    let mut agents: Vec<String> = identity_path()
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| toml::from_str::<IdentityFile>(&s).ok())
        .map(|f| f.agents)
        .unwrap_or_default();

    // Merge in the configured agents (agents.toml keys) that can actually run.
    for (name, command) in crate::agent::config::AgentConfig::load().agents.iter() {
        if !advertisable(&command.command) {
            continue;
        }
        if !agents.iter().any(|a| a == name) {
            agents.push(name.clone());
        }
    }
    agents
}

/// Whether a configured launch command can actually start on this machine, and
/// so may be advertised to peers as runnable. See [`read_local_agents`] for why
/// each condition is here.
fn advertisable(command: &[String]) -> bool {
    let program = command.first().map(String::as_str).unwrap_or("");
    crate::agent::plugin::command_status(command) != "missing"
        && crate::agent::probe::bridge_ready(program)
}

/// These tests access private fields (`seed`, `IdentityFile`) and must live
/// inline. All tests using only the public API live in `tests/identity.rs`.
#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    // An entry with no command cannot launch, so peers must not be offered it.
    #[test]
    fn an_empty_command_is_not_advertisable() {
        assert!(!advertisable(&cmd(&[])));
    }

    // The condition that catches a self-hosting ACP agent (`suzent acp`) whose
    // product CLI is absent: command[0] is the CLI itself, not an adapter, so
    // nothing else would notice it missing.
    #[test]
    fn a_missing_program_is_not_advertisable() {
        assert!(!advertisable(&cmd(&[
            "definitely-not-a-real-agent-xyz",
            "acp"
        ])));
    }

    // A program that resolves here is advertisable. The test binary is an
    // absolute, executable path on every platform, unlike any PATH name.
    #[test]
    fn a_resolvable_program_is_advertisable() {
        let exe = std::env::current_exe().expect("test binary has a path");
        assert!(advertisable(&cmd(&[&exe.to_string_lossy(), "acp"])));
    }

    // Deliberate: Device Settings calls `npx …` not-ready to nudge migration,
    // but it can still start, so unadvertising it would break working legacy
    // configs rather than protect anyone.
    #[test]
    fn a_runtime_download_command_stays_advertisable() {
        assert_eq!(
            crate::agent::plugin::command_status(&cmd(&["npx", "some-acp-agent"])),
            "runtime_download"
        );
        assert!(advertisable(&cmd(&["npx", "some-acp-agent"])));
    }

    /// The recovery phrase lives only in the file, so every `save` has to carry
    /// it forward. It used not to: `enox identity set-label` and receiving a
    /// link both rewrote identity.toml without it, destroying the only copy of
    /// the user root key with no way back.
    #[test]
    fn saving_does_not_destroy_the_recovery_phrase() {
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("ENOXIAN_HOME", home.path());

        let (user, mnemonic) = UserIdentity::generate("suzy".into()).unwrap();
        let mut device = DeviceIdentity::generate("first-machine".into());
        device.save().unwrap();
        user.link_device(&mut device, &mnemonic).unwrap();

        let stored = |()| -> Option<String> {
            let raw = std::fs::read_to_string(home.path().join("identity.toml")).unwrap();
            toml::from_str::<IdentityFile>(&raw).unwrap().user_mnemonic
        };
        assert_eq!(stored(()).as_deref(), Some(mnemonic.as_str()));

        // Any ordinary save — a rename, adopting a handle, receiving a link.
        device.device_label = "renamed".into();
        device.save().unwrap();
        assert_eq!(
            stored(()).as_deref(),
            Some(mnemonic.as_str()),
            "a plain save erased the recovery phrase"
        );

        // And the device can still act as the root holder afterwards.
        assert!(device.user_identity().unwrap().is_some());
        std::env::remove_var("ENOXIAN_HOME");
    }

    /// The label is a display name the user can change. Binding it into the
    /// signature meant renaming a device permanently invalidated its
    /// attestation, with no way to reissue one.
    #[test]
    fn renaming_a_device_does_not_invalidate_its_attestation() {
        let (user, _) = UserIdentity::generate("suzy".into()).unwrap();
        let mut device = DeviceIdentity::generate("before".into());
        let device_pubkey =
            hex::encode(device.device_keypair().unwrap().public().encode_protobuf());
        let attestation = user.attest_device(&device_pubkey).unwrap();

        device.device_label = "after".into();
        assert!(UserIdentity::verify_attestation(
            &user.pubkey_hex().unwrap(),
            &device_pubkey,
            &attestation
        )
        .unwrap());
    }

    /// The signed bytes must name exactly one device key. The old encoding was
    /// `pubkey_bytes || label` with no length prefix, so an attestation for
    /// (key `AABB`, label `CC`) covered the same bytes as one for (key
    /// `AABBCC`, label ``) — one signature, two device identities.
    #[test]
    fn an_attestation_cannot_be_reinterpreted_as_another_key() {
        let (user, _) = UserIdentity::generate("suzy".into()).unwrap();
        let device = DeviceIdentity::generate("machine".into());
        let key = device.device_keypair().unwrap().public().encode_protobuf();
        let attestation = user.attest_device(&hex::encode(&key)).unwrap();
        let user_pubkey = user.pubkey_hex().unwrap();

        // The same bytes with something appended must not verify.
        let mut extended = key.clone();
        extended.push(0x61);
        assert!(
            !UserIdentity::verify_attestation(&user_pubkey, &hex::encode(&extended), &attestation)
                .unwrap_or(false),
            "a longer key reused the signature"
        );
    }

    /// A device key has to be a real key. Without that check the root key will
    /// sign whatever bytes a peer sends during `enox link`.
    #[test]
    fn the_root_key_refuses_to_sign_something_that_is_not_a_key() {
        let (user, _) = UserIdentity::generate("suzy".into()).unwrap();
        assert!(user
            .attest_device(&hex::encode(b"not a key at all"))
            .is_err());
        assert!(user.attest_device("").is_err());
        assert!(user.attest_device("nothex").is_err());
    }

    /// An attestation that does not cover this device must never reach disk —
    /// storing one unchecked is invisible until something relies on it.
    #[test]
    fn adopting_refuses_an_attestation_for_a_different_device() {
        let (user, _) = UserIdentity::generate("suzy".into()).unwrap();
        let other = DeviceIdentity::generate("someone-else".into());
        let other_pubkey = hex::encode(other.device_keypair().unwrap().public().encode_protobuf());
        let attestation = user.attest_device(&other_pubkey).unwrap();

        let mut mine = DeviceIdentity::generate("mine".into());
        let err = mine
            .adopt_attestation("suzy".into(), user.pubkey_hex().unwrap(), attestation)
            .unwrap_err();
        assert!(err.to_string().contains("does not cover"), "got: {err}");
        assert!(
            mine.user_pubkey_hex.is_none(),
            "nothing should have been set"
        );
    }

    /// An attestation must verify against the user key that signed it, and
    /// against nothing else. `enox link` sends one to another device, so a
    /// signature over the wrong bytes would be discovered only much later.
    #[test]
    fn an_attestation_verifies_against_the_user_key_that_signed_it() {
        let (user, _) = UserIdentity::generate("suzy".into()).unwrap();
        let device = DeviceIdentity::generate("suzy-laptop".into());
        let device_pubkey =
            hex::encode(device.device_keypair().unwrap().public().encode_protobuf());

        let attestation = user.attest_device(&device_pubkey).unwrap();
        assert!(UserIdentity::verify_attestation(
            &user.pubkey_hex().unwrap(),
            &device_pubkey,
            &attestation
        )
        .unwrap());
    }

    /// An attestation names one device and one user. Neither end may be swapped
    /// for another's, or it would transfer between machines or identities.
    #[test]
    fn an_attestation_does_not_transfer_to_another_device_or_user() {
        let (user, _) = UserIdentity::generate("suzy".into()).unwrap();
        let device = DeviceIdentity::generate("suzy-laptop".into());
        let other = DeviceIdentity::generate("someone-else".into());
        let device_pubkey =
            hex::encode(device.device_keypair().unwrap().public().encode_protobuf());
        let other_pubkey = hex::encode(other.device_keypair().unwrap().public().encode_protobuf());
        let attestation = user.attest_device(&device_pubkey).unwrap();
        let user_pubkey = user.pubkey_hex().unwrap();

        assert!(
            !UserIdentity::verify_attestation(&user_pubkey, &other_pubkey, &attestation).unwrap(),
            "device key swapped"
        );

        let (other_user, _) = UserIdentity::generate("mallory".into()).unwrap();
        assert!(
            !UserIdentity::verify_attestation(
                &other_user.pubkey_hex().unwrap(),
                &device_pubkey,
                &attestation
            )
            .unwrap(),
            "user key swapped"
        );
    }

    /// `adopt_attestation` is what a linked device runs. It must take on the
    /// user identity without acquiring the root key — that is the whole point
    /// of sending an attestation rather than the mnemonic.
    #[test]
    fn adopting_an_attestation_does_not_bring_the_root_key_with_it() {
        let (user, _) = UserIdentity::generate("suzy".into()).unwrap();
        let mut device = DeviceIdentity::generate("new-machine".into());
        let device_pubkey =
            hex::encode(device.device_keypair().unwrap().public().encode_protobuf());
        let attestation = user.attest_device(&device_pubkey).unwrap();

        device
            .adopt_attestation(
                "suzy".into(),
                user.pubkey_hex().unwrap(),
                attestation.clone(),
            )
            .unwrap();

        assert_eq!(device.user_handle.as_deref(), Some("suzy"));
        assert!(UserIdentity::verify_attestation(
            device.user_pubkey_hex.as_ref().unwrap(),
            &device_pubkey,
            &attestation
        )
        .unwrap());
        // The root key is not among what it took on: adopting sets the handle,
        // the user public key and the signature, and nothing else. That the
        // mnemonic survives a save is `save`'s job, covered separately.
        assert!(device.user_attestation_hex.is_some());
    }

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
                agents: device.agents.clone(),
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
            agents: Vec::new(),
        };
        assert_eq!(
            pid_before,
            d2.derive_circle_keypair("c1")
                .unwrap()
                .public()
                .to_peer_id()
        );
    }
}
