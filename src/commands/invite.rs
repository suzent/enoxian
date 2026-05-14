use anyhow::{Context, Result};
use chrono::Utc;

use crate::{
    cli::InviteArgs,
    config::{circle_dir, load_all},
    crypto::keypair_from_hex,
    invite::{self, InvitePayload},
    resolve,
};

pub async fn run(args: InviteArgs) -> Result<()> {
    let configs = load_all()?;
    let config = resolve::resolve(&args.circle, &configs)
        .with_context(|| format!("circle '{}' not found — run `enoch circles` to list known circles", args.circle))?
        .clone();

    let psk_bytes = hex::decode(&config.psk_hex)
        .context("config.toml has invalid psk_hex")?;
    let psk: [u8; 32] = psk_bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("psk_hex must be 32 bytes (64 hex chars)"))?;

    let ttl = invite::parse_ttl(&args.ttl)?;
    let expires_at = Utc::now() + ttl;

    // Embed admin pubkey if admin.key is present (only on admin machines)
    let admin_pubkey_bytes = try_load_admin_pubkey(&config.circle_id);

    let uri = invite::encode(&InvitePayload {
        circle_id:   config.circle_id.clone(),
        psk_bytes:   psk,
        circle_name: Some(config.circle_name.clone()),
        expires_at,
        peer_addr:   args.peer.clone(),
        admin_pubkey_bytes,
    });

    println!("✦ Invite for '{}' (valid {}):", config.circle_name, args.ttl);
    println!();
    println!("  {uri}");
    println!();
    println!("  Join with: enoch enter \"<invite>\"");

    Ok(())
}

fn try_load_admin_pubkey(circle_id: &str) -> Option<Vec<u8>> {
    let key_path = circle_dir(circle_id).ok()?.join("admin.key");
    let hex = std::fs::read_to_string(&key_path).ok()?;
    let keypair = keypair_from_hex(hex.trim()).ok()?;
    Some(keypair.public().encode_protobuf())
}
