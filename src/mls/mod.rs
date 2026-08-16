pub mod group;
pub mod identity;

pub use group::MlsGroupManager;
pub use identity::MlsIdentity;

pub use openmls::prelude::Ciphersuite;

pub const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;

use std::sync::Arc;
use tokio::sync::Mutex;

pub struct MlsState {
    pub identity: MlsIdentity,
    pub group: Option<MlsGroupManager>,
}

pub type SharedMlsState = Arc<Mutex<MlsState>>;

pub fn new_mls_state(identity: MlsIdentity, group: Option<MlsGroupManager>) -> SharedMlsState {
    Arc::new(Mutex::new(MlsState { identity, group }))
}
