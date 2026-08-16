//! Integration tests for the identity module — public API only.
//!
//! Tests that require private field access (seed, IdentityFile) stay in
//! `src/identity.rs`. Tests here exercise DeviceIdentity and UserIdentity
//! through the same surface area that external callers use.

use enoxian::identity::{DeviceIdentity, UserIdentity};

// ── Key derivation ────────────────────────────────────────────────────────────

#[test]
fn circle_keypair_is_deterministic() {
    let d = DeviceIdentity::generate("test".to_string());
    let pid1 = d
        .derive_circle_keypair("circle-abc")
        .unwrap()
        .public()
        .to_peer_id();
    let pid2 = d
        .derive_circle_keypair("circle-abc")
        .unwrap()
        .public()
        .to_peer_id();
    assert_eq!(
        pid1, pid2,
        "same device + same circle_id must always produce the same peer ID"
    );
}

#[test]
fn different_circles_produce_different_peer_ids() {
    let d = DeviceIdentity::generate("test".to_string());
    let pid_a = d
        .derive_circle_keypair("circle-alpha")
        .unwrap()
        .public()
        .to_peer_id();
    let pid_b = d
        .derive_circle_keypair("circle-beta")
        .unwrap()
        .public()
        .to_peer_id();
    assert_ne!(
        pid_a, pid_b,
        "different circle IDs must yield different peer IDs"
    );
}

#[test]
fn different_devices_produce_different_peer_ids_for_same_circle() {
    let d1 = DeviceIdentity::generate("device-one".to_string());
    let d2 = DeviceIdentity::generate("device-two".to_string());
    let pid1 = d1
        .derive_circle_keypair("shared-circle")
        .unwrap()
        .public()
        .to_peer_id();
    let pid2 = d2
        .derive_circle_keypair("shared-circle")
        .unwrap()
        .public()
        .to_peer_id();
    assert_ne!(
        pid1, pid2,
        "different devices must produce different peer IDs for the same circle"
    );
}

#[test]
fn device_keypair_differs_from_circle_keypair() {
    let d = DeviceIdentity::generate("test".to_string());
    let device_pid = d.device_keypair().unwrap().public().to_peer_id();
    let circle_pid = d
        .derive_circle_keypair("some-circle")
        .unwrap()
        .public()
        .to_peer_id();
    assert_ne!(device_pid, circle_pid);
}

// ── Display name ──────────────────────────────────────────────────────────────

#[test]
fn display_name_falls_back_to_device_label() {
    let d = DeviceIdentity::generate("my-laptop".to_string());
    assert_eq!(d.display_name(), "my-laptop");
}

#[test]
fn display_name_prefers_user_handle_over_device_label() {
    let mut d = DeviceIdentity::generate("my-laptop".to_string());
    d.set_user_handle("suzy".to_string());
    assert_eq!(d.display_name(), "suzy");
}

// ── User identity & mnemonic ──────────────────────────────────────────────────

#[test]
fn generated_mnemonic_is_24_words() {
    let (_user, mnemonic) = UserIdentity::generate("alice".to_string()).unwrap();
    assert_eq!(mnemonic.split_whitespace().count(), 24);
}

#[test]
fn mnemonic_round_trip_recovers_same_pubkey() {
    let (user, mnemonic) = UserIdentity::generate("alice".to_string()).unwrap();
    let pk1 = user.pubkey_hex().unwrap();

    let user2 = UserIdentity::from_mnemonic(&mnemonic, "alice".to_string()).unwrap();
    let pk2 = user2.pubkey_hex().unwrap();

    assert_eq!(
        pk1, pk2,
        "restoring from mnemonic must reproduce the same user public key"
    );
}

#[test]
fn invalid_mnemonic_is_rejected() {
    assert!(
        UserIdentity::from_mnemonic("not a valid mnemonic phrase here", "x".to_string()).is_err()
    );
}

#[test]
fn two_users_with_different_seeds_produce_different_pubkeys() {
    let (u1, _) = UserIdentity::generate("alice".to_string()).unwrap();
    let (u2, _) = UserIdentity::generate("alice".to_string()).unwrap();
    assert_ne!(u1.pubkey_hex().unwrap(), u2.pubkey_hex().unwrap());
}

// ── Attestation ───────────────────────────────────────────────────────────────

#[test]
fn attestation_signature_is_64_bytes() {
    let (user, _) = UserIdentity::generate("alice".to_string()).unwrap();
    let device = DeviceIdentity::generate("laptop".to_string());
    let device_pubkey = hex::encode(device.device_keypair().unwrap().public().encode_protobuf());
    let attestation = user
        .attest_device(&device_pubkey, &device.device_label)
        .unwrap();
    // Ed25519 signature = 64 bytes → 128 hex chars
    assert_eq!(
        attestation.len(),
        128,
        "attestation must be a 64-byte Ed25519 signature"
    );
}

#[test]
fn different_users_produce_different_attestations_for_same_device() {
    let device = DeviceIdentity::generate("laptop".to_string());
    let device_pubkey = hex::encode(device.device_keypair().unwrap().public().encode_protobuf());

    let (u1, _) = UserIdentity::generate("alice".to_string()).unwrap();
    let (u2, _) = UserIdentity::generate("bob".to_string()).unwrap();

    let att1 = u1
        .attest_device(&device_pubkey, &device.device_label)
        .unwrap();
    let att2 = u2
        .attest_device(&device_pubkey, &device.device_label)
        .unwrap();
    assert_ne!(att1, att2);
}

// ── Integration: identity → circle key → peer ID stability ───────────────────

#[test]
fn peer_id_survives_handle_update() {
    // Updating user handle or label must not change the derived circle keypair.
    let mut d = DeviceIdentity::generate("my-device".to_string());
    let pid_before = d
        .derive_circle_keypair("circle-x")
        .unwrap()
        .public()
        .to_peer_id();

    d.set_user_handle("suzy".to_string());
    let pid_after = d
        .derive_circle_keypair("circle-x")
        .unwrap()
        .public()
        .to_peer_id();

    assert_eq!(
        pid_before, pid_after,
        "changing user handle must not change the circle keypair"
    );
}

#[test]
fn many_circles_all_produce_distinct_peer_ids() {
    let d = DeviceIdentity::generate("device".to_string());
    let circles = ["alpha", "beta", "gamma", "delta", "epsilon"];
    let peer_ids: Vec<_> = circles
        .iter()
        .map(|c| d.derive_circle_keypair(c).unwrap().public().to_peer_id())
        .collect();

    // All must be distinct.
    for i in 0..peer_ids.len() {
        for j in (i + 1)..peer_ids.len() {
            assert_ne!(
                peer_ids[i], peer_ids[j],
                "circles {} and {} collided",
                circles[i], circles[j]
            );
        }
    }
}
