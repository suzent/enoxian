use anyhow::{Context, Result};
use chrono::Utc;

use crate::{
    cli::InviteArgs,
    config::load,
    invite::{self, InvitePayload},
};

pub async fn run(args: InviteArgs) -> Result<()> {
    let config = load(&args.circle_id)
        .with_context(|| format!("circle '{}' not found — run `enoch init` first", args.circle_id))?;

    let psk_bytes = hex::decode(&config.psk_hex)
        .context("config.toml has invalid psk_hex")?;
    let psk: [u8; 32] = psk_bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("psk_hex must be 32 bytes (64 hex chars)"))?;

    let ttl = invite::parse_ttl(&args.ttl)?;
    let expires_at = Utc::now() + ttl;

    let uri = invite::encode(&InvitePayload {
        circle_id:   config.circle_id.clone(),
        psk_bytes:   psk,
        circle_name: Some(config.circle_name.clone()),
        expires_at,
        peer_addr:   args.peer.clone(),
    });

    println!("✦ Invite for '{}' (valid {}):", config.circle_name, args.ttl);
    println!();
    println!("  {uri}");
    println!();
    println!("  Join with: enoch enter \"<invite>\"");

    Ok(())
}
