use anyhow::{bail, Result};
use libp2p::identity::Keypair;
use rand::RngCore;

pub fn generate_psk() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes
}

#[allow(dead_code)] // used in Phase 1 (pnet transport PSK)
pub fn psk_from_hex(s: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(s.trim())?;
    if bytes.len() != 32 {
        bail!("PSK must be 32 bytes (64 hex chars), got {}", bytes.len());
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(arr)
}

pub fn generate_keypair() -> Keypair {
    Keypair::generate_ed25519()
}

pub fn keypair_to_hex(keypair: &Keypair) -> Result<String> {
    let bytes = keypair
        .to_protobuf_encoding()
        .map_err(|e| anyhow::anyhow!("keypair encoding failed: {e}"))?;
    Ok(hex::encode(bytes))
}

pub fn keypair_from_hex(s: &str) -> Result<Keypair> {
    let bytes = hex::decode(s.trim())?;
    Keypair::from_protobuf_encoding(&bytes)
        .map_err(|e| anyhow::anyhow!("keypair decoding failed: {e}"))
}
