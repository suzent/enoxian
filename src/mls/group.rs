use anyhow::{Context, Result};
use base64::prelude::*;
use openmls::prelude::*;
use openmls::prelude::GroupId;
use std::collections::HashMap;
use std::path::Path;
use tls_codec::{Deserialize as _, Serialize as _};

use super::{identity::MlsIdentity, CIPHERSUITE};

const PSK_LABEL: &str = "enochian-psk";
const PSK_LEN: usize = 32;

pub struct MlsGroupManager {
    pub group: MlsGroup,
}

impl MlsGroupManager {
    // ── Creator: called once at `enoch init` ──────────────────────────────────

    pub fn create(identity: &MlsIdentity) -> Result<Self> {
        let config = MlsGroupCreateConfig::builder()
            .ciphersuite(CIPHERSUITE)
            .use_ratchet_tree_extension(true)
            .build();

        let group = MlsGroup::new(
            &identity.provider,
            &identity.signer,
            &config,
            identity.credential_with_key.clone(),
        )
        .map_err(|e| anyhow::anyhow!("create MLS group: {e:?}"))?;

        Ok(Self { group })
    }

    // ── Joiner: called when daemon finds mls_welcomes[our_peer_id] ────────────
    // welcome_bytes: TLS-serialized MlsMessageOut wrapping a Welcome
    // ratchet_tree_bytes: TLS-serialized RatchetTree (sent alongside Welcome)

    pub fn join_from_welcome(
        identity: &MlsIdentity,
        welcome_bytes: &[u8],
        ratchet_tree_bytes: Option<&[u8]>,
    ) -> Result<Self> {
        let mut b = welcome_bytes;
        let msg = MlsMessageIn::tls_deserialize(&mut b)
            .context("deserialize Welcome message")?;

        let welcome = match msg.extract() {
            MlsMessageBodyIn::Welcome(w) => w,
            _ => return Err(anyhow::anyhow!("expected Welcome message")),
        };

        let ratchet_tree = ratchet_tree_bytes
            .map(|bytes| {
                let mut b = bytes;
                RatchetTreeIn::tls_deserialize(&mut b)
                    .context("deserialize ratchet tree")
            })
            .transpose()?;

        let group = StagedWelcome::new_from_welcome(
            &identity.provider,
            &MlsGroupJoinConfig::default(),
            welcome,
            ratchet_tree,
        )
        .map_err(|e| anyhow::anyhow!("stage Welcome: {e:?}"))?
        .into_group(&identity.provider)
        .map_err(|e| anyhow::anyhow!("join from Welcome: {e:?}"))?;

        Ok(Self { group })
    }

    // ── Admin: add a member ───────────────────────────────────────────────────
    // key_package_bytes: TLS-serialized KeyPackage published by the new member
    // Returns (commit_bytes, welcome_bytes, ratchet_tree_bytes)

    pub fn add_member(
        &mut self,
        identity: &MlsIdentity,
        key_package_bytes: &[u8],
    ) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
        let mut b = key_package_bytes;
        let key_package_in = KeyPackageIn::tls_deserialize(&mut b)
            .context("deserialize KeyPackage")?;

        let key_package = key_package_in
            .validate(identity.provider.crypto(), ProtocolVersion::Mls10)
            .map_err(|e| anyhow::anyhow!("invalid KeyPackage: {e:?}"))?;

        let (commit, welcome_msg, _group_info) = self
            .group
            .add_members(&identity.provider, &identity.signer, &[key_package])
            .map_err(|e| anyhow::anyhow!("MLS add_members: {e:?}"))?;

        self.group
            .merge_pending_commit(&identity.provider)
            .map_err(|e| anyhow::anyhow!("merge add commit: {e:?}"))?;

        let commit_bytes = commit
            .tls_serialize_detached()
            .context("serialize Commit")?;

        // Serialize the full MlsMessageOut (Welcome wire format); join side
        // deserializes as MlsMessageIn and extracts via extract().
        let welcome_bytes = welcome_msg
            .tls_serialize_detached()
            .context("serialize Welcome")?;

        let ratchet_tree_bytes = self
            .group
            .export_ratchet_tree()
            .tls_serialize_detached()
            .context("serialize ratchet tree")?;

        Ok((commit_bytes, welcome_bytes, ratchet_tree_bytes))
    }

    // ── Admin: remove a member ────────────────────────────────────────────────

    pub fn remove_member(
        &mut self,
        identity: &MlsIdentity,
        leaf_index: u32,
    ) -> Result<Vec<u8>> {
        let (commit, _, _) = self
            .group
            .remove_members(
                &identity.provider,
                &identity.signer,
                &[LeafNodeIndex::new(leaf_index)],
            )
            .map_err(|e| anyhow::anyhow!("MLS remove_members: {e:?}"))?;

        self.group
            .merge_pending_commit(&identity.provider)
            .map_err(|e| anyhow::anyhow!("merge remove commit: {e:?}"))?;

        commit
            .tls_serialize_detached()
            .context("serialize Commit")
    }

    // ── Non-admin: apply an incoming Commit ───────────────────────────────────

    pub fn apply_commit(
        &mut self,
        identity: &MlsIdentity,
        commit_bytes: &[u8],
    ) -> Result<()> {
        let mut b = commit_bytes;
        let message = MlsMessageIn::tls_deserialize(&mut b)
            .context("deserialize Commit message")?;

        let protocol_message = message
            .try_into_protocol_message()
            .map_err(|_| anyhow::anyhow!("MLS message is not a protocol message"))?;

        let processed = self
            .group
            .process_message(&identity.provider, protocol_message)
            .map_err(|e| anyhow::anyhow!("process Commit: {e:?}"))?;

        if let ProcessedMessageContent::StagedCommitMessage(staged) =
            processed.into_content()
        {
            self.group
                .merge_staged_commit(&identity.provider, *staged)
                .map_err(|e| anyhow::anyhow!("merge staged commit: {e:?}"))?;
        }

        Ok(())
    }

    // ── Derive the pnet PSK from the current MLS epoch ────────────────────────

    pub fn epoch_psk(&self, identity: &MlsIdentity) -> Result<[u8; 32]> {
        let raw = self
            .group
            .export_secret(identity.provider.crypto(), PSK_LABEL, &[], PSK_LEN)
            .map_err(|e| anyhow::anyhow!("MLS export_secret: {e:?}"))?;

        raw.try_into()
            .map_err(|_| anyhow::anyhow!("exporter returned wrong length"))
    }

    pub fn epoch(&self) -> u64 {
        self.group.epoch().as_u64()
    }

    // ── Leaf index of a member by peer_id ─────────────────────────────────────

    pub fn leaf_index_for_peer(&self, peer_id: &str) -> Option<u32> {
        let target = peer_id.as_bytes();
        self.group.members().find_map(|m| {
            let bc = BasicCredential::try_from(m.credential).ok()?;
            if bc.identity() == target {
                Some(m.index.u32())
            } else {
                None
            }
        })
    }

    pub fn save(&self, identity: &MlsIdentity, circle_dir: &Path) -> Result<()> {
        let group_id_bytes = self.group.group_id().as_slice().to_vec();
        let group_id_b64 = BASE64_STANDARD.encode(&group_id_bytes);

        let storage_map: HashMap<String, String> = {
            let values = identity.provider.storage().values.read()
                .map_err(|_| anyhow::anyhow!("storage lock poisoned"))?;
            values.iter()
                .map(|(k, v)| (BASE64_STANDARD.encode(k), BASE64_STANDARD.encode(v)))
                .collect()
        };

        let json = serde_json::json!({
            "group_id": group_id_b64,
            "storage": storage_map,
        });

        let mls_dir = circle_dir.join("mls");
        std::fs::create_dir_all(&mls_dir)
            .with_context(|| format!("create {}", mls_dir.display()))?;
        std::fs::write(mls_dir.join("group.json"), serde_json::to_string_pretty(&json)?)
            .context("write group.json")?;
        Ok(())
    }

    pub fn load(identity: &MlsIdentity, circle_dir: &Path) -> Result<Option<Self>> {
        let path = circle_dir.join("mls").join("group.json");
        if !path.exists() {
            return Ok(None);
        }

        let json: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&path).context("read group.json")?
        ).context("parse group.json")?;

        let group_id_b64 = json["group_id"].as_str()
            .context("missing group_id in group.json")?;
        let group_id_bytes = BASE64_STANDARD.decode(group_id_b64)
            .context("decode group_id")?;
        let group_id = GroupId::from_slice(&group_id_bytes);

        {
            let storage_obj = json["storage"].as_object()
                .context("missing storage in group.json")?;
            let mut values = identity.provider.storage().values.write()
                .map_err(|_| anyhow::anyhow!("storage lock poisoned"))?;
            for (k_b64, v_b64) in storage_obj {
                let k = BASE64_STANDARD.decode(k_b64).context("decode storage key")?;
                let v = BASE64_STANDARD.decode(v_b64.as_str().context("storage value not string")?)
                    .context("decode storage value")?;
                values.insert(k, v);
            }
        }

        let group = MlsGroup::load(identity.provider.storage(), &group_id)
            .map_err(|e| anyhow::anyhow!("load MLS group: {e:?}"))?;

        Ok(group.map(|g| Self { group: g }))
    }
}
