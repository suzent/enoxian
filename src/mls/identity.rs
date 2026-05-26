use anyhow::{Context, Result};
use openmls::prelude::*;
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tls_codec::Serialize as _;

use super::CIPHERSUITE;

// ── Persisted form ────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct StoredIdentity {
    /// serde_json of the SignatureKeyPair (openmls_basic_credential supports this)
    signer_json: serde_json::Value,
    credential_with_key_json: serde_json::Value,
    peer_id: String,
}

// ── Public type ───────────────────────────────────────────────────────────────

pub struct MlsIdentity {
    pub provider: OpenMlsRustCrypto,
    pub credential_with_key: CredentialWithKey,
    pub signer: SignatureKeyPair,
    pub peer_id: String,
}

impl MlsIdentity {
    fn mls_dir(circle_dir: &Path) -> PathBuf {
        circle_dir.join("mls")
    }

    fn identity_path(circle_dir: &Path) -> PathBuf {
        Self::mls_dir(circle_dir).join("identity.json")
    }

    // ── Generate a brand-new identity ─────────────────────────────────────────

    pub fn generate(peer_id: &str) -> Result<Self> {
        let provider = OpenMlsRustCrypto::default();

        let signer = SignatureKeyPair::new(CIPHERSUITE.signature_algorithm())
            .context("generate MLS signing keypair")?;
        signer
            .store(provider.storage())
            .context("store MLS signer in key store")?;

        let credential = BasicCredential::new(peer_id.as_bytes().to_vec());
        let credential_with_key = CredentialWithKey {
            credential: credential.into(),
            signature_key: signer.to_public_vec().into(),
        };

        Ok(Self {
            provider,
            credential_with_key,
            signer,
            peer_id: peer_id.to_string(),
        })
    }

    // ── Persist to circle_dir/mls/identity.json ───────────────────────────────

    pub fn save(&self, circle_dir: &Path) -> Result<()> {
        let mls_dir = Self::mls_dir(circle_dir);
        std::fs::create_dir_all(&mls_dir)
            .with_context(|| format!("create {}", mls_dir.display()))?;

        let stored = StoredIdentity {
            signer_json: serde_json::to_value(&self.signer)?,
            credential_with_key_json: serde_json::to_value(&self.credential_with_key)?,
            peer_id: self.peer_id.clone(),
        };
        let json = serde_json::to_string_pretty(&stored)?;
        std::fs::write(Self::identity_path(circle_dir), json)?;
        Ok(())
    }

    // ── Load from disk, or generate + save if not present ─────────────────────

    pub fn load_or_generate(circle_dir: &Path, peer_id: &str) -> Result<Self> {
        let id_path = Self::identity_path(circle_dir);
        if id_path.exists() {
            return Self::load(circle_dir);
        }
        let identity = Self::generate(peer_id)?;
        identity.save(circle_dir)?;
        Ok(identity)
    }

    fn load(circle_dir: &Path) -> Result<Self> {
        let json = std::fs::read_to_string(Self::identity_path(circle_dir))?;
        let stored: StoredIdentity = serde_json::from_str(&json)?;

        let signer: SignatureKeyPair = serde_json::from_value(stored.signer_json)
            .context("deserialize MLS signer")?;
        let credential_with_key: CredentialWithKey =
            serde_json::from_value(stored.credential_with_key_json)
                .context("deserialize MLS credential")?;

        let provider = OpenMlsRustCrypto::default();
        signer
            .store(provider.storage())
            .context("store loaded MLS signer")?;

        Ok(Self {
            provider,
            credential_with_key,
            signer,
            peer_id: stored.peer_id,
        })
    }

    // ── Generate a KeyPackage (serialised, ready for distribution) ────────────

    pub fn generate_key_package(&self) -> Result<Vec<u8>> {
        let bundle = KeyPackage::builder()
            .build(CIPHERSUITE, &self.provider, &self.signer, self.credential_with_key.clone())
            .map_err(|e| anyhow::anyhow!("build KeyPackage: {e:?}"))?;

        bundle
            .key_package()
            .tls_serialize_detached()
            .context("serialize KeyPackage")
    }
}
