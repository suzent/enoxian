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
    /// A single attestation signed by the user root key, as written before
    /// chains existed. Read on load and folded into `attestation_chain`; never
    /// written again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_attestation_hex: Option<String>,
    /// The signatures carrying the user root's authority down to this device.
    ///
    /// One link when the root key attested this device directly, two when a
    /// device that was itself linked did it, and so on — which is what lets a
    /// linked device link the next one without the root key ever moving.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attestation_chain: Vec<ChainLink>,
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
    /// See [`IdentityFile::attestation_chain`]. Empty when this device has not
    /// been linked to a user.
    pub attestation_chain: Vec<ChainLink>,
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
            attestation_chain: Vec::new(),
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
            user_attestation_hex: None,
            attestation_chain: self.attestation_chain.clone(),
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

        // Fold a pre-chain attestation into a one-link chain, so a device
        // written by an older build keeps working and is upgraded the next time
        // it saves.
        let attestation_chain = migrate_chain(&file, &seed)?;

        Ok(DeviceIdentity {
            seed,
            device_label: file.device_label,
            user_handle: file.user_handle,
            user_pubkey_hex: file.user_pubkey_hex,
            attestation_chain,
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

    /// Whether a recovery phrase is still sitting in this device's identity file.
    ///
    /// Nothing needs it any more — attestation chains let any linked device
    /// vouch for the next one — so it is no longer written. Installs made before
    /// that still have one, and it is the single worst thing on the disk: with
    /// it, whoever finds the machine *is* the user, on every device, for good.
    ///
    /// Reported rather than deleted. Removing it silently would destroy the only
    /// copy for anyone who never wrote the words down; `enox identity
    /// forget-phrase` is how they say they have.
    pub fn stored_phrase(&self) -> Option<String> {
        let path = identity_path().ok()?;
        let raw = std::fs::read_to_string(path).ok()?;
        toml::from_str::<IdentityFile>(&raw).ok()?.user_mnemonic
    }

    /// Drop a stored recovery phrase, keeping everything else about the device.
    pub fn forget_phrase(&self) -> Result<()> {
        let path = identity_path()?;
        let raw = std::fs::read_to_string(&path).context("read identity.toml")?;
        let mut file: IdentityFile = toml::from_str(&raw).context("parse identity.toml")?;
        file.user_mnemonic = None;
        crate::config::write_secret(&path, toml::to_string_pretty(&file)?)
    }

    /// Adopt a user identity that another device vouched for this one.
    ///
    /// Deliberately takes a chain of signatures rather than the root key: the
    /// device that holds the root keeps the only copy, and this device gets an
    /// identity of its own that can be revoked without touching the others.
    ///
    /// The chain is checked here, against this device's own key, so a device
    /// cannot end up storing an identity it has no proof of. Storing one
    /// unchecked would be invisible until something relied on it.
    pub fn adopt_chain(
        &mut self,
        handle: String,
        user_pubkey_hex: String,
        chain: Vec<ChainLink>,
    ) -> Result<()> {
        let device_pubkey_hex = self.device_pubkey_hex()?;
        if !verify_chain(&chain, &user_pubkey_hex, &device_pubkey_hex)? {
            bail!(
                "the attestation from the other device does not cover this device's key — \
                 nothing was saved"
            );
        }
        self.user_handle = Some(handle);
        self.user_pubkey_hex = Some(user_pubkey_hex);
        self.attestation_chain = chain;
        Ok(())
    }

    /// hex(protobuf) of this device's own public key.
    pub fn device_pubkey_hex(&self) -> Result<String> {
        Ok(hex::encode(
            self.device_keypair()?.public().encode_protobuf(),
        ))
    }

    /// Whether this device can prove it belongs to the user it names.
    pub fn attestation_is_valid(&self) -> bool {
        let (Some(user_pubkey), Ok(device_pubkey)) =
            (self.user_pubkey_hex.as_deref(), self.device_pubkey_hex())
        else {
            return false;
        };
        verify_chain(&self.attestation_chain, user_pubkey, &device_pubkey).unwrap_or(false)
    }

    /// Vouch for another device, extending this device's own chain by one link.
    ///
    /// This is what lets a device that was itself linked link the next one. It
    /// signs with the device key, not the root key — which it does not have —
    /// so the result is a longer chain rather than a second root-signed
    /// attestation. Refuses if this device cannot prove its own standing,
    /// because a chain built on an unprovable link proves nothing either.
    pub fn attest(&self, subject_pubkey_hex: &str) -> Result<Vec<ChainLink>> {
        if !self.attestation_is_valid() {
            bail!("this device has no valid attestation of its own to extend");
        }
        if self.attestation_chain.len() + 1 > MAX_CHAIN_DEPTH {
            bail!(
                "this device is already {} links from the user root; link the new device \
                 from one closer to it",
                self.attestation_chain.len()
            );
        }

        let msg = attestation_message(subject_pubkey_hex)?;
        let sig = self
            .device_keypair()?
            .sign(&msg)
            .map_err(|e| anyhow::anyhow!("signing an attestation failed: {e}"))?;

        let mut chain = self.attestation_chain.clone();
        chain.push(ChainLink {
            signer_pubkey_hex: self.device_pubkey_hex()?,
            subject_pubkey_hex: subject_pubkey_hex.trim().to_string(),
            sig: hex::encode(sig),
        });
        Ok(chain)
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

    /// Link this user to a device identity (mutates device; saves it).
    ///
    /// The recovery phrase is deliberately not persisted. It used to be kept on
    /// the device the identity was created on, because only a device holding the
    /// root key could attest another — attestation chains removed that need, and
    /// what is left is a phrase on disk that turns a lost laptop into a lost
    /// identity. It is shown once, to be written down, and never written here.
    pub fn link_device(&self, device: &mut DeviceIdentity) -> Result<()> {
        let device_pubkey = device.device_pubkey_hex()?;
        device.user_handle = Some(self.handle.clone());
        device.user_pubkey_hex = Some(self.pubkey_hex()?);
        device.attestation_chain = vec![ChainLink {
            signer_pubkey_hex: self.pubkey_hex()?,
            subject_pubkey_hex: device_pubkey.clone(),
            sig: self.attest_device(&device_pubkey)?,
        }];
        device.save()
    }
}

// ── Attestation ───────────────────────────────────────────────────────────────

/// Domain tag on an attestation, so a signature made here can never be read as
/// one made by some other part of the system with the same key.
const ATTESTATION_DOMAIN: &[u8] = b"enoxian-device-attestation-v1";

/// How many signatures a chain may carry, root included.
///
/// A chain exists so a device that was itself linked can link the next one
/// without the root key. In practice everything is one or two hops from the
/// root; the cap is here so a malformed or hostile chain cannot be walked
/// forever, not because four is a meaningful number of devices.
pub const MAX_CHAIN_DEPTH: usize = 4;

/// The exact bytes an attestation covers.
///
/// Length-prefixed, and over the device key alone. The label is deliberately
/// not signed: it is a display name the user can change, and binding it meant
/// `enox identity set-label` silently invalidated the device's attestation.
fn attestation_message(device_pubkey_hex: &str) -> Result<Vec<u8>> {
    let device_bytes = hex::decode(device_pubkey_hex.trim()).context("decode device pubkey")?;
    // A device key is a libp2p public key. Refusing anything else keeps a
    // signing key from being talked into signing bytes of somebody's choosing.
    libp2p::identity::PublicKey::try_decode_protobuf(&device_bytes)
        .map_err(|e| anyhow::anyhow!("device public key is not a valid key: {e}"))?;

    let len = u32::try_from(device_bytes.len()).context("device pubkey is implausibly long")?;
    let mut msg = Vec::with_capacity(ATTESTATION_DOMAIN.len() + 4 + device_bytes.len());
    msg.extend_from_slice(ATTESTATION_DOMAIN);
    msg.extend_from_slice(&len.to_be_bytes());
    msg.extend_from_slice(&device_bytes);
    Ok(msg)
}

/// One signature in an attestation chain: `signer` vouches for `subject`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChainLink {
    /// hex(protobuf) of the key that signed.
    pub signer_pubkey_hex: String,
    /// hex(protobuf) of the device key being vouched for.
    pub subject_pubkey_hex: String,
    /// hex of the signature over [`attestation_message`] for `subject`.
    pub sig: String,
}

impl ChainLink {
    /// Whether this link's signature really is the signer's, over this subject.
    pub fn is_valid(&self) -> Result<bool> {
        let signer_bytes =
            hex::decode(self.signer_pubkey_hex.trim()).context("decode signer pubkey")?;
        let signer = libp2p::identity::PublicKey::try_decode_protobuf(&signer_bytes)
            .map_err(|e| anyhow::anyhow!("invalid signer public key: {e}"))?;
        let sig = hex::decode(self.sig.trim()).context("decode attestation")?;
        Ok(signer.verify(&attestation_message(&self.subject_pubkey_hex)?, &sig))
    }
}

/// Check that `chain` carries `user_pubkey_hex`'s authority down to
/// `device_pubkey_hex`.
///
/// Every link must be signed by the key the previous link vouched for, the
/// first must be signed by the user root itself, and the last must name this
/// device. A key may appear as a subject only once, so a chain cannot be padded
/// with a cycle to get past the depth cap.
pub fn verify_chain(
    chain: &[ChainLink],
    user_pubkey_hex: &str,
    device_pubkey_hex: &str,
) -> Result<bool> {
    if chain.is_empty() || chain.len() > MAX_CHAIN_DEPTH {
        return Ok(false);
    }

    let mut seen: Vec<&str> = Vec::with_capacity(chain.len());
    let mut expected_signer = user_pubkey_hex.trim();

    for link in chain {
        if link.signer_pubkey_hex.trim() != expected_signer {
            return Ok(false);
        }
        let subject = link.subject_pubkey_hex.trim();
        if seen.contains(&subject) || subject == expected_signer {
            return Ok(false);
        }
        if !link.is_valid()? {
            return Ok(false);
        }
        seen.push(subject);
        expected_signer = subject;
    }

    Ok(expected_signer == device_pubkey_hex.trim())
}

/// Turn an older identity file's single attestation into a one-link chain.
///
/// The old field was a bare signature with the signer implied to be the user
/// root, so the link is reconstructed from `user_pubkey_hex` and this device's
/// own key. Returns the stored chain untouched when there already is one.
fn migrate_chain(file: &IdentityFile, seed: &[u8; 32]) -> Result<Vec<ChainLink>> {
    if !file.attestation_chain.is_empty() {
        return Ok(file.attestation_chain.clone());
    }
    let (Some(user_pubkey_hex), Some(sig)) = (
        file.user_pubkey_hex.clone(),
        file.user_attestation_hex.clone(),
    ) else {
        return Ok(Vec::new());
    };
    Ok(vec![ChainLink {
        signer_pubkey_hex: user_pubkey_hex,
        subject_pubkey_hex: hex::encode(device_public_key(seed)?.encode_protobuf()),
        sig,
    }])
}

/// This device's public key from its seed alone — needed during load, before a
/// `DeviceIdentity` exists to ask.
fn device_public_key(seed: &[u8; 32]) -> Result<libp2p::identity::PublicKey> {
    let hk = Hkdf::<Sha256>::new(Some(b"enoxian-device-v1"), seed);
    let mut okm = [0u8; 32];
    hk.expand(b"circle/__device__", &mut okm)
        .map_err(|_| anyhow::anyhow!("HKDF expand failed"))?;
    let secret = libp2p::identity::ed25519::SecretKey::try_from_bytes(okm)
        .map_err(|e| anyhow::anyhow!("ed25519 secret: {e}"))?;
    Ok(Keypair::from(libp2p::identity::ed25519::Keypair::from(secret)).public())
}

// ── Device / circle-key binding ───────────────────────────────────────────────

/// Domain tag on the signature linking a circle key to the device that derived
/// it. Distinct from [`ATTESTATION_DOMAIN`] so the two can never be confused.
const BINDING_DOMAIN: &[u8] = b"enoxian-circle-device-binding-v1";

/// The bytes a device signs to adopt a per-circle key as its own.
///
/// A circle key is derived from the device seed through HKDF, so the circle
/// *public* key cannot be recovered from the device public key — there is no
/// arithmetic link between a peer ID and the device behind it. This signature
/// supplies one.
///
/// The direction matters, and getting it backwards is a full identity spoof.
/// Signed by the **device** key over the **circle** key, it says "that device
/// authorises this peer ID", and only someone holding the device's private key
/// can say it. Signed the other way — by the circle key over the device key —
/// it would say only "this peer claims that device", which any member can claim
/// about anyone: a circle key is the claimant's own, and the device key, user
/// key and chain are all published in the control doc. An attacker could then
/// pair a victim's public material with a binding made by their own circle key
/// and be returned as the victim.
///
/// Scoped to one circle, so a binding published in a circle the user has left
/// cannot be replayed into another.
fn binding_message(circle_id: &str, circle_pubkey: &[u8]) -> Result<Vec<u8>> {
    let circle = circle_id.trim().as_bytes();
    let mut msg = Vec::with_capacity(BINDING_DOMAIN.len() + 8 + circle.len() + circle_pubkey.len());
    msg.extend_from_slice(BINDING_DOMAIN);
    msg.extend_from_slice(&(circle.len() as u32).to_be_bytes());
    msg.extend_from_slice(circle);
    msg.extend_from_slice(&(circle_pubkey.len() as u32).to_be_bytes());
    msg.extend_from_slice(circle_pubkey);
    Ok(msg)
}

/// Sign, with this device's key, the adoption of `circle_keypair` for
/// `circle_id`.
pub fn sign_binding(
    device_keypair: &Keypair,
    circle_id: &str,
    circle_keypair: &Keypair,
) -> Result<String> {
    let msg = binding_message(circle_id, &circle_keypair.public().encode_protobuf())?;
    let sig = device_keypair
        .sign(&msg)
        .map_err(|e| anyhow::anyhow!("signing the device binding failed: {e}"))?;
    Ok(hex::encode(sig))
}

/// Whether the device named by `device_pubkey_hex` authorised `peer_id` for
/// this circle.
///
/// Verified with the **device** key, over the circle key recovered from the
/// peer ID itself. Both halves matter:
///
/// - the signature is checked against the device key, so producing one needs
///   that device's private key — the attacker's own circle key buys nothing;
/// - the circle key comes from the peer ID rather than from the claim, so
///   nobody supplies the key their own signature is checked against.
pub fn verify_binding(
    peer_id: &str,
    circle_id: &str,
    device_pubkey_hex: &str,
    binding_hex: &str,
) -> Result<bool> {
    let peer: libp2p::PeerId = peer_id
        .trim()
        .parse()
        .map_err(|e| anyhow::anyhow!("not a peer id: {e}"))?;
    let Some(circle_key) = peer_public_key(&peer) else {
        // Only keys small enough to be embedded can be recovered this way.
        // Every circle key is Ed25519, so anything else did not come from here.
        return Ok(false);
    };

    let device_bytes = hex::decode(device_pubkey_hex.trim()).context("decode device pubkey")?;
    let Ok(device_key) = libp2p::identity::PublicKey::try_decode_protobuf(&device_bytes) else {
        return Ok(false);
    };

    let sig = hex::decode(binding_hex.trim()).context("decode device binding")?;
    let msg = binding_message(circle_id, &circle_key.encode_protobuf())?;
    Ok(device_key.verify(&msg, &sig))
}

/// Recover the public key an Ed25519 peer ID carries inline.
fn peer_public_key(peer: &libp2p::PeerId) -> Option<libp2p::identity::PublicKey> {
    let hash = libp2p::multihash::Multihash::from(*peer);
    // 0x00 is the identity multihash: the "digest" is the key itself.
    if hash.code() != 0x00 {
        return None;
    }
    libp2p::identity::PublicKey::try_decode_protobuf(hash.digest()).ok()
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

    /// `ENOXIAN_HOME` is process-wide, so two tests that repoint it will read
    /// each other's directory when the harness runs them in parallel. Anything
    /// touching the real identity file has to take this first.
    fn with_home<T>(body: impl FnOnce(&std::path::Path) -> T) -> T {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        // A panicking test poisons the lock; the env var is reset either way,
        // so later tests are still safe to run.
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let home = tempfile::tempdir().unwrap();
        std::env::set_var("ENOXIAN_HOME", home.path());
        let out = body(home.path());
        std::env::remove_var("ENOXIAN_HOME");
        out
    }

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
    /// The recovery phrase is no longer written anywhere. It existed on disk so
    /// the device that created the identity could attest another; chains made
    /// that unnecessary, and what was left was a phrase whose presence turned a
    /// lost laptop into a lost identity.
    #[test]
    fn creating_a_user_does_not_write_the_phrase_to_disk() {
        with_home(|home| {
            let (user, mnemonic) = UserIdentity::generate("suzy".into()).unwrap();
            let mut device = DeviceIdentity::generate("first-machine".into());
            device.save().unwrap();
            user.link_device(&mut device).unwrap();

            let raw = std::fs::read_to_string(home.join("identity.toml")).unwrap();
            assert!(
                !raw.contains(&mnemonic),
                "the recovery phrase was written to identity.toml"
            );
            assert!(device.stored_phrase().is_none());

            // And the device can still vouch for the next one without it.
            assert!(device.attestation_is_valid());
            let next = DeviceIdentity::generate("second".into());
            assert!(device.attest(&next.device_pubkey_hex().unwrap()).is_ok());
        });
    }

    /// An install made before this still has a phrase on disk, and an ordinary
    /// save must not quietly destroy it — that would take the only copy from
    /// someone who never wrote the words down. It goes when they say so.
    #[test]
    fn an_existing_phrase_survives_until_it_is_forgotten() {
        with_home(|home| {
            let (user, mnemonic) = UserIdentity::generate("suzy".into()).unwrap();
            let mut device = DeviceIdentity::generate("older-install".into());
            device.save().unwrap();
            user.link_device(&mut device).unwrap();

            // Stand in for a file written by the older build.
            let path = home.join("identity.toml");
            let mut file: IdentityFile =
                toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            file.user_mnemonic = Some(mnemonic.clone());
            std::fs::write(&path, toml::to_string_pretty(&file).unwrap()).unwrap();

            let device = DeviceIdentity::load().unwrap();
            assert_eq!(device.stored_phrase().as_deref(), Some(mnemonic.as_str()));

            // Ordinary saves leave it alone.
            let mut renamed = device.clone();
            renamed.device_label = "renamed".into();
            renamed.save().unwrap();
            assert_eq!(
                DeviceIdentity::load().unwrap().stored_phrase().as_deref(),
                Some(mnemonic.as_str()),
                "a plain save erased a phrase the user may not have written down"
            );

            // Forgetting removes the phrase and nothing else.
            renamed.forget_phrase().unwrap();
            let after = DeviceIdentity::load().unwrap();
            assert!(after.stored_phrase().is_none());
            assert_eq!(after.device_label, "renamed");
            assert!(after.attestation_is_valid(), "the identity itself survived");
        });
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
            .adopt_chain(
                "suzy".into(),
                user.pubkey_hex().unwrap(),
                vec![ChainLink {
                    signer_pubkey_hex: user.pubkey_hex().unwrap(),
                    subject_pubkey_hex: other_pubkey.clone(),
                    sig: attestation,
                }],
            )
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
            .adopt_chain(
                "suzy".into(),
                user.pubkey_hex().unwrap(),
                vec![ChainLink {
                    signer_pubkey_hex: user.pubkey_hex().unwrap(),
                    subject_pubkey_hex: device_pubkey.clone(),
                    sig: attestation.clone(),
                }],
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
        assert!(device.attestation_is_valid());
    }

    // ── Attestation chains ───────────────────────────────────────────────────

    fn linked(user: &UserIdentity, label: &str) -> DeviceIdentity {
        let mut d = DeviceIdentity::generate(label.into());
        let pk = d.device_pubkey_hex().unwrap();
        d.user_handle = Some(user.handle.clone());
        d.user_pubkey_hex = Some(user.pubkey_hex().unwrap());
        d.attestation_chain = vec![ChainLink {
            signer_pubkey_hex: user.pubkey_hex().unwrap(),
            subject_pubkey_hex: pk,
            sig: user.attest_device(&d.device_pubkey_hex().unwrap()).unwrap(),
        }];
        d
    }

    /// The point of chains: a device that was itself linked can link the next
    /// one, without the root key ever moving. Previously only the device
    /// holding the mnemonic could attest anything.
    #[test]
    fn a_linked_device_can_vouch_for_a_third() {
        let (user, _) = UserIdentity::generate("suzy".into()).unwrap();
        let laptop = linked(&user, "laptop");
        assert!(laptop.attestation_is_valid());
        assert!(
            laptop.stored_phrase().is_none(),
            "a linked device must not hold the recovery phrase"
        );

        let mut phone = DeviceIdentity::generate("phone".into());
        let chain = laptop.attest(&phone.device_pubkey_hex().unwrap()).unwrap();
        assert_eq!(chain.len(), 2, "root -> laptop -> phone");

        phone
            .adopt_chain("suzy".into(), user.pubkey_hex().unwrap(), chain)
            .unwrap();
        assert!(phone.attestation_is_valid());
    }

    /// A device with nothing to extend must not manufacture authority.
    #[test]
    fn an_unlinked_device_cannot_vouch_for_anything() {
        let lonely = DeviceIdentity::generate("nobody".into());
        let other = DeviceIdentity::generate("other".into());
        let err = lonely
            .attest(&other.device_pubkey_hex().unwrap())
            .unwrap_err();
        assert!(
            err.to_string().contains("no valid attestation"),
            "got: {err}"
        );
    }

    /// A chain must actually reach the device it is presented for.
    #[test]
    fn a_chain_for_another_device_does_not_verify() {
        let (user, _) = UserIdentity::generate("suzy".into()).unwrap();
        let laptop = linked(&user, "laptop");
        let phone = DeviceIdentity::generate("phone".into());
        let stranger = DeviceIdentity::generate("stranger".into());

        let chain = laptop.attest(&phone.device_pubkey_hex().unwrap()).unwrap();
        assert!(!verify_chain(
            &chain,
            &user.pubkey_hex().unwrap(),
            &stranger.device_pubkey_hex().unwrap()
        )
        .unwrap());
    }

    /// The chain must start at the user root. A chain that starts anywhere else
    /// is a device vouching for itself with extra steps.
    #[test]
    fn a_chain_that_does_not_start_at_the_user_root_is_refused() {
        let (user, _) = UserIdentity::generate("suzy".into()).unwrap();
        let (mallory, _) = UserIdentity::generate("mallory".into()).unwrap();
        let laptop = linked(&mallory, "laptop");
        let phone = DeviceIdentity::generate("phone".into());
        let chain = laptop.attest(&phone.device_pubkey_hex().unwrap()).unwrap();

        // The chain is internally consistent, but rooted in the wrong user.
        assert!(verify_chain(
            &chain,
            &mallory.pubkey_hex().unwrap(),
            &phone.device_pubkey_hex().unwrap()
        )
        .unwrap());
        assert!(!verify_chain(
            &chain,
            &user.pubkey_hex().unwrap(),
            &phone.device_pubkey_hex().unwrap()
        )
        .unwrap());
    }

    /// A broken link anywhere invalidates everything below it, or a chain could
    /// be spliced together from signatures that were never issued as a chain.
    #[test]
    fn a_tampered_link_breaks_the_chain() {
        let (user, _) = UserIdentity::generate("suzy".into()).unwrap();
        let laptop = linked(&user, "laptop");
        let phone = DeviceIdentity::generate("phone".into());
        let phone_pk = phone.device_pubkey_hex().unwrap();

        for i in 0..2 {
            let mut chain = laptop.attest(&phone_pk).unwrap();
            chain[i].sig = hex::encode([0u8; 64]);
            assert!(
                !verify_chain(&chain, &user.pubkey_hex().unwrap(), &phone_pk).unwrap(),
                "link {i} went unnoticed"
            );
        }

        // Dropping the root link must not leave a chain that verifies either.
        let mut chain = laptop.attest(&phone_pk).unwrap();
        chain.remove(0);
        assert!(!verify_chain(&chain, &user.pubkey_hex().unwrap(), &phone_pk).unwrap());
    }

    /// A chain may not be padded with a cycle to walk past the depth cap, and
    /// an over-long chain is refused outright.
    #[test]
    fn a_chain_cannot_cycle_or_run_past_the_cap() {
        let (user, _) = UserIdentity::generate("suzy".into()).unwrap();
        let laptop = linked(&user, "laptop");
        let laptop_pk = laptop.device_pubkey_hex().unwrap();

        // A link naming the laptop again, after it already appeared.
        let mut chain = laptop.attest(&laptop_pk).unwrap_or_default();
        if chain.is_empty() {
            chain = laptop.attestation_chain.clone();
            chain.push(ChainLink {
                signer_pubkey_hex: laptop_pk.clone(),
                subject_pubkey_hex: laptop_pk.clone(),
                sig: hex::encode([0u8; 64]),
            });
        }
        assert!(!verify_chain(&chain, &user.pubkey_hex().unwrap(), &laptop_pk).unwrap());

        let too_long = vec![chain[0].clone(); MAX_CHAIN_DEPTH + 1];
        assert!(!verify_chain(&too_long, &user.pubkey_hex().unwrap(), &laptop_pk).unwrap());
        assert!(!verify_chain(&[], &user.pubkey_hex().unwrap(), &laptop_pk).unwrap());
    }

    // ── Circle-key binding ───────────────────────────────────────────────────

    /// The binding is what ties a peer id to a device, since a circle key is
    /// HKDF-derived and cannot be linked to a device key by arithmetic.
    #[test]
    fn a_binding_ties_a_peer_id_to_the_device_behind_it() {
        let device = DeviceIdentity::generate("laptop".into());
        let circle = "8e563c41-f0ec-4225-9764-064f1fb04341";
        let circle_kp = device.derive_circle_keypair(circle).unwrap();
        let peer_id = circle_kp.public().to_peer_id().to_string();
        let device_pk = device.device_pubkey_hex().unwrap();

        let binding = sign_binding(&device.device_keypair().unwrap(), circle, &circle_kp).unwrap();
        assert!(verify_binding(&peer_id, circle, &device_pk, &binding).unwrap());
    }

    /// The attack the first version of this allowed, and the reason the
    /// signature is made by the device key rather than the circle key.
    ///
    /// Everything a peer publishes about its identity — user key, device key,
    /// attestation chain — is readable by every member. If the binding were
    /// signed by the *circle* key, a member could pair that public material with
    /// a binding made by their own circle key and be returned as the victim.
    /// Signing with the device key means the proof needs the victim's device
    /// private key, which is never published.
    #[test]
    fn a_member_cannot_bind_someone_elses_device_to_their_own_peer() {
        let circle = "8e563c41-f0ec-4225-9764-064f1fb04341";
        let victim = DeviceIdentity::generate("victim-laptop".into());
        let victim_device_pk = victim.device_pubkey_hex().unwrap();

        // An ordinary member of the same circle, with their own circle key.
        let attacker = DeviceIdentity::generate("attacker".into());
        let attacker_circle_kp = attacker.derive_circle_keypair(circle).unwrap();
        let attacker_peer = attacker_circle_kp.public().to_peer_id().to_string();

        // Everything they can copy from the control doc, plus a binding they
        // sign themselves with the one key they do control.
        let forged = sign_binding(
            &attacker.device_keypair().unwrap(),
            circle,
            &attacker_circle_kp,
        )
        .unwrap();
        assert!(
            !verify_binding(&attacker_peer, circle, &victim_device_pk, &forged).unwrap(),
            "an attacker bound the victim's device to their own peer"
        );

        // Nor by signing with the circle key, which is what the broken version
        // verified against.
        let circle_signed = {
            let msg =
                binding_message(circle, &attacker_circle_kp.public().encode_protobuf()).unwrap();
            hex::encode(attacker_circle_kp.sign(&msg).unwrap())
        };
        assert!(
            !verify_binding(&attacker_peer, circle, &victim_device_pk, &circle_signed).unwrap(),
            "a circle-key signature was accepted for someone else's device"
        );
    }

    /// A binding is scoped to one circle and one device: it must not carry over
    /// to another circle, another peer, or another device key.
    #[test]
    fn a_binding_does_not_travel() {
        let device = DeviceIdentity::generate("laptop".into());
        let other_device = DeviceIdentity::generate("other".into());
        let circle = "8e563c41-f0ec-4225-9764-064f1fb04341";
        let circle_kp = device.derive_circle_keypair(circle).unwrap();
        let peer_id = circle_kp.public().to_peer_id().to_string();
        let device_pk = device.device_pubkey_hex().unwrap();
        let device_kp = device.device_keypair().unwrap();
        let binding = sign_binding(&device_kp, circle, &circle_kp).unwrap();

        assert!(
            !verify_binding(&peer_id, "another-circle", &device_pk, &binding).unwrap(),
            "replayed into another circle"
        );
        assert!(
            !verify_binding(
                &peer_id,
                circle,
                &other_device.device_pubkey_hex().unwrap(),
                &binding
            )
            .unwrap(),
            "claimed a different device"
        );

        let stranger_peer = other_device
            .derive_circle_keypair(circle)
            .unwrap()
            .public()
            .to_peer_id()
            .to_string();
        assert!(
            !verify_binding(&stranger_peer, circle, &device_pk, &binding).unwrap(),
            "another peer reused the signature"
        );

        // A binding the device made for one of its own circle keys must not be
        // reusable for a different circle key of the same device.
        let sibling = device.derive_circle_keypair("sibling-circle").unwrap();
        let sibling_peer = sibling.public().to_peer_id().to_string();
        assert!(
            !verify_binding(&sibling_peer, circle, &device_pk, &binding).unwrap(),
            "reused across the device's own circle keys"
        );
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
                attestation_chain: device.attestation_chain.clone(),
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
            attestation_chain: Vec::new(),
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
