use chrono::{DateTime, Duration, Utc};
use dashmap::DashMap;
use rand::RngCore;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub const DEFAULT_TOKEN_TTL_HOURS: i64 = 1;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ActorIdentity {
    pub registration_id: String,
    pub agent_id: String,
    pub circle_id: String,
    pub peer_id: String,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Default)]
pub struct ActorTokenRegistry {
    entries: Arc<DashMap<String, ActorIdentity>>,
}

impl ActorTokenRegistry {
    pub fn issue(&self, circle_id: &str, peer_id: &str, agent_id: &str) -> (String, ActorIdentity) {
        let now = Utc::now();
        let identity = ActorIdentity {
            registration_id: uuid::Uuid::new_v4().to_string(),
            agent_id: agent_id.to_string(),
            circle_id: circle_id.to_string(),
            peer_id: peer_id.to_string(),
            issued_at: now,
            expires_at: now + Duration::hours(DEFAULT_TOKEN_TTL_HOURS),
        };

        // Include the device and Circle in token derivation, while retaining
        // 256 bits of randomness. Public peer IDs alone must never be bearer
        // credentials.
        let mut random = [0_u8; 32];
        rand::thread_rng().fill_bytes(&mut random);
        let mut digest = Sha256::new();
        digest.update(b"enoxian-actor-token-v1\0");
        digest.update(circle_id.as_bytes());
        digest.update(b"\0");
        digest.update(peer_id.as_bytes());
        digest.update(b"\0");
        digest.update(random);
        let token = format!("enox_at_{}", hex::encode(digest.finalize()));
        self.entries.insert(token_hash(&token), identity.clone());
        (token, identity)
    }

    pub fn validate(
        &self,
        token: &str,
        circle_id: &str,
        peer_id: &str,
    ) -> Result<ActorIdentity, ActorTokenError> {
        let key = token_hash(token);
        let Some(entry) = self.entries.get(&key) else {
            return Err(ActorTokenError::Invalid);
        };
        if entry.expires_at <= Utc::now() {
            drop(entry);
            self.entries.remove(&key);
            return Err(ActorTokenError::Expired);
        }
        if entry.circle_id != circle_id || entry.peer_id != peer_id {
            return Err(ActorTokenError::WrongDeviceOrCircle);
        }
        Ok(entry.clone())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ActorTokenError {
    Invalid,
    Expired,
    WrongDeviceOrCircle,
}

fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_bound_to_circle_and_device() {
        let registry = ActorTokenRegistry::default();
        let (token, issued) = registry.issue("circle-a", "peer-a", "codex");
        let actor = registry.validate(&token, "circle-a", "peer-a").unwrap();
        assert_eq!(actor.agent_id, "codex");
        assert_eq!(actor.registration_id, issued.registration_id);
        assert_eq!(
            registry.validate(&token, "circle-b", "peer-a"),
            Err(ActorTokenError::WrongDeviceOrCircle)
        );
        assert_eq!(
            registry.validate(&token, "circle-a", "peer-b"),
            Err(ActorTokenError::WrongDeviceOrCircle)
        );
        assert_eq!(
            registry.validate("enox_at_not-the-token", "circle-a", "peer-a"),
            Err(ActorTokenError::Invalid)
        );
    }

    #[test]
    fn expired_token_is_rejected_and_removed() {
        let registry = ActorTokenRegistry::default();
        let (token, _) = registry.issue("circle-a", "peer-a", "hermes");
        let key = token_hash(&token);
        registry.entries.get_mut(&key).unwrap().expires_at = Utc::now() - Duration::seconds(1);

        assert_eq!(
            registry.validate(&token, "circle-a", "peer-a"),
            Err(ActorTokenError::Expired)
        );
        assert!(!registry.entries.contains_key(&key));
    }
}
