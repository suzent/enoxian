pub mod group;
pub mod identity;

pub use group::MlsGroupManager;
pub use identity::MlsIdentity;

pub use openmls::prelude::Ciphersuite;

pub const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;

use std::sync::Arc;
use std::{collections::BTreeMap, io};
use tokio::sync::Mutex;

pub struct MlsState {
    pub identity: MlsIdentity,
    pub group: Option<MlsGroupManager>,
    /// Recently used exporter secrets, retained only in memory so a frame sent
    /// just before an epoch transition can still be opened after the commit is
    /// applied. Offline catch-up uses MLS commits, not persisted old secrets.
    epoch_secrets: BTreeMap<u64, [u8; 32]>,
}

pub type SharedMlsState = Arc<Mutex<MlsState>>;

/// A peer's place in the MLS group; see [`MlsState::standing`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// No leaf for this peer.
    Absent,
    /// A leaf carrying the keys the peer publishes: already admitted.
    Current,
    /// A leaf left over from before the peer rejoined with new keys.
    Stale,
}

pub fn new_mls_state(identity: MlsIdentity, group: Option<MlsGroupManager>) -> SharedMlsState {
    let mut state = MlsState {
        identity,
        group,
        epoch_secrets: BTreeMap::new(),
    };
    let _ = state.refresh_content_secret();
    Arc::new(Mutex::new(state))
}

impl MlsState {
    pub fn current_epoch(&self) -> Option<u64> {
        self.group.as_ref().map(MlsGroupManager::epoch)
    }

    /// Where `peer_id` stands in the group, judged against the KeyPackage it
    /// has published.
    ///
    /// A leaf alone does not prove admission. A device that left the circle and
    /// entered it again keeps its peer id but not its MLS keys, so its old leaf
    /// is still in the tree while the device cannot decrypt a thing. A leaf
    /// whose signature key differs from the published KeyPackage is that case.
    pub fn standing(&self, peer_id: &str, key_package_bytes: Option<&[u8]>) -> Standing {
        let Some((_, leaf_key)) = self
            .group
            .as_ref()
            .and_then(|group| group.leaf_signature_key_for_peer(peer_id))
        else {
            return Standing::Absent;
        };
        // No KeyPackage, or one that does not parse: nothing shows the leaf is
        // stale, and approval would fail on it anyway.
        let published = key_package_bytes
            .and_then(|bytes| group::validate_key_package(&self.identity, bytes).ok());
        match published {
            Some(kp) if kp.leaf_node().signature_key().as_slice() != leaf_key.as_slice() => {
                Standing::Stale
            }
            _ => Standing::Current,
        }
    }

    /// Admit `peer_id` with its published KeyPackage, returning
    /// `(commit, welcome, ratchet_tree)`: a plain add for a newcomer, or one
    /// commit that swaps out the leaf of a device that rejoined.
    ///
    /// Lives here so the split borrow of `identity` against `group` stays
    /// inside the module; callers previously reached for a raw pointer and
    /// `unsafe` to express the same thing.
    ///
    /// **This advances the group epoch and cannot be undone.** The commit it
    /// returns is the only way other devices can follow, so a caller that fails
    /// to publish it strands every peer on the old epoch. Publish it in the same
    /// unit of work that calls this.
    pub fn admit_member(
        &mut self,
        peer_id: &str,
        key_package_bytes: &[u8],
    ) -> anyhow::Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
        let Self {
            identity, group, ..
        } = self;
        let group = group
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("MLS group not initialized"))?;
        match group.leaf_signature_key_for_peer(peer_id) {
            Some((leaf_index, _)) => group.readmit_member(identity, leaf_index, key_package_bytes),
            None => group.add_member(identity, key_package_bytes),
        }
    }

    /// Remove a member by peer id, returning `(commit, epoch)`.
    ///
    /// `Ok(None)` means there was nothing to do at the MLS layer — no group, or
    /// a peer that never joined it — which is a CRDT-only eviction, not a
    /// failure. Like [`Self::admit_member`] this advances the epoch irreversibly
    /// when it does act, so publish the commit in the same unit of work.
    pub fn remove_member_by_peer(
        &mut self,
        peer_id: &str,
    ) -> anyhow::Result<Option<(Vec<u8>, u64)>> {
        let Self {
            identity, group, ..
        } = self;
        let Some(group) = group.as_mut() else {
            return Ok(None);
        };
        let Some(leaf_index) = group.leaf_index_for_peer(peer_id) else {
            return Ok(None);
        };
        let commit = group.remove_member(identity, leaf_index)?;
        Ok(Some((commit, group.epoch())))
    }

    /// Persist the group to disk. A no-op when no group is initialized.
    pub fn save(&self, circle_dir: &std::path::Path) -> anyhow::Result<()> {
        match &self.group {
            Some(group) => group.save(&self.identity, circle_dir),
            None => Ok(()),
        }
    }

    pub fn refresh_content_secret(&mut self) -> anyhow::Result<Option<(u64, [u8; 32])>> {
        let Some(group) = self.group.as_ref() else {
            return Ok(None);
        };
        let epoch = group.epoch();
        let secret = group.content_secret(&self.identity)?;
        self.epoch_secrets.insert(epoch, secret);
        while self.epoch_secrets.len() > 8 {
            let Some(oldest) = self.epoch_secrets.keys().next().copied() else {
                break;
            };
            self.epoch_secrets.remove(&oldest);
        }
        Ok(Some((epoch, secret)))
    }

    pub fn content_secret_for_epoch(&mut self, epoch: u64) -> anyhow::Result<[u8; 32]> {
        if let Some(secret) = self.epoch_secrets.get(&epoch) {
            return Ok(*secret);
        }
        if self.current_epoch() == Some(epoch) {
            return self
                .refresh_content_secret()?
                .map(|(_, secret)| secret)
                .ok_or_else(|| anyhow::anyhow!("MLS group unavailable"));
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("MLS content secret for epoch {epoch} unavailable"),
        )
        .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_without_group() -> MlsState {
        MlsState {
            identity: MlsIdentity::generate("peer-local").unwrap(),
            group: None,
            epoch_secrets: BTreeMap::new(),
        }
    }

    /// The approval and eviction handlers branch on these three contracts to
    /// decide whether an operation touched the MLS layer at all. A circle with
    /// no group must be an ordinary CRDT-only path, not an error path.
    #[test]
    fn removing_a_peer_without_a_group_is_a_no_op_not_a_failure() {
        let mut state = state_without_group();
        assert!(
            matches!(state.remove_member_by_peer("12D3KooWabsent"), Ok(None)),
            "no MLS group means a CRDT-only eviction, which is not a failure"
        );
    }

    #[test]
    fn saving_without_a_group_succeeds() {
        let state = state_without_group();
        assert!(
            state.save(std::path::Path::new("/nonexistent")).is_ok(),
            "there is nothing to persist, so there is nothing to fail"
        );
    }

    #[test]
    fn adding_a_member_without_a_group_is_an_error() {
        let mut state = state_without_group();
        assert!(
            state
                .admit_member("12D3KooWabsent", b"not-a-key-package")
                .is_err(),
            "approval must not silently succeed when there is no group to add to"
        );
    }

    fn state_with_group(peer_id: &str) -> MlsState {
        let identity = MlsIdentity::generate(peer_id).unwrap();
        let group = MlsGroupManager::create(&identity).unwrap();
        MlsState {
            identity,
            group: Some(group),
            epoch_secrets: BTreeMap::new(),
        }
    }

    /// A device that leaves a circle deletes its MLS state, then enters again
    /// under the same peer id with fresh keys. Treating its old leaf as proof of
    /// admission left it pending forever: connected, but never given a key.
    #[test]
    fn a_device_that_rejoins_with_new_keys_is_readmitted() {
        let peer = "12D3KooWrejoiner";
        let mut admin = state_with_group("12D3KooWadmin");
        let bystander = MlsIdentity::generate("12D3KooWbystander").unwrap();

        let first = MlsIdentity::generate(peer).unwrap();
        let first_package = first.generate_key_package().unwrap();
        assert_eq!(admin.standing(peer, Some(&first_package)), Standing::Absent);
        admin.admit_member(peer, &first_package).unwrap();
        let (_, bystander_welcome, _) = admin
            .admit_member(
                "12D3KooWbystander",
                &bystander.generate_key_package().unwrap(),
            )
            .unwrap();
        let mut bystander_group =
            MlsGroupManager::join_from_welcome(&bystander, &bystander_welcome, None).unwrap();

        // A restart republishes a KeyPackage from the same identity: still a member.
        let restarted_package = first.generate_key_package().unwrap();
        assert_eq!(
            admin.standing(peer, Some(&restarted_package)),
            Standing::Current
        );
        assert_eq!(admin.standing(peer, None), Standing::Current);

        // Leave and enter again: same peer id, new identity.
        let second = MlsIdentity::generate(peer).unwrap();
        let second_package = second.generate_key_package().unwrap();
        assert_eq!(admin.standing(peer, Some(&second_package)), Standing::Stale);

        let (commit, welcome, _) = admin.admit_member(peer, &second_package).unwrap();
        let rejoined = MlsGroupManager::join_from_welcome(&second, &welcome, None).unwrap();
        bystander_group.apply_commit(&bystander, &commit).unwrap();

        assert_eq!(
            admin.standing(peer, Some(&second_package)),
            Standing::Current
        );
        let group = admin.group.as_ref().unwrap();
        assert_eq!(
            group
                .group
                .members()
                .filter(
                    |m| openmls::prelude::BasicCredential::try_from(m.credential.clone())
                        .is_ok_and(|c| c.identity() == peer.as_bytes())
                )
                .count(),
            1,
            "the old leaf is replaced, not kept alongside the new one"
        );
        let secret = group.content_secret(&admin.identity).unwrap();
        assert_eq!(secret, rejoined.content_secret(&second).unwrap());
        assert_eq!(secret, bystander_group.content_secret(&bystander).unwrap());
    }
}
