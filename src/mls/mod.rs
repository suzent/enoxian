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
