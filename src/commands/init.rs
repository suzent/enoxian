use anyhow::{bail, Result};
use chrono::Utc;
use uuid::Uuid;

use crate::{
    cli::InitArgs,
    config::{self, circle_dir, default_workspace_dir, CircleConfig},
    crypto::{generate_keypair, generate_psk, keypair_to_hex},
    invite::{self, InvitePayload},
};

pub async fn run(args: InitArgs) -> Result<()> {
    // ── Enforce unique name locally ───────────────────────────────────────────
    let existing = config::load_all()?;
    if existing.iter().any(|c| c.circle_name == args.name) {
        bail!(
            "a circle named '{}' already exists — run `enoch circles` to list existing circles, or choose a different name",
            args.name
        );
    }

    // ── Resolve workspace directory ───────────────────────────────────────────
    let workspace_dir = match args.dir {
        Some(d) => d,
        None => default_workspace_dir(&args.name)?,
    };
    tokio::fs::create_dir_all(&workspace_dir).await?;

    // ── Generate credentials ──────────────────────────────────────────────────
    let circle_id = Uuid::new_v4().to_string();
    let psk = generate_psk();
    let keypair = generate_keypair();
    let peer_id = keypair.public().to_peer_id();

    // Admin keypair — generated now; enforcement added in M6.
    // Private key lives only on this machine; public key is shared in config.
    let admin_keypair = generate_keypair();
    let admin_pubkey_hex = hex::encode(admin_keypair.public().encode_protobuf());
    let admin_privkey_hex = keypair_to_hex(&admin_keypair)?;

    let config = CircleConfig {
        circle_id:         circle_id.clone(),
        circle_name:       args.name.clone(),
        psk_hex:           hex::encode(psk),
        keypair_proto_hex: keypair_to_hex(&keypair)?,
        workspace_dir:     workspace_dir.to_string_lossy().into_owned(),
        admin_pubkey_hex:  admin_pubkey_hex.clone(),
    };
    config::save(&config)?;

    // Save admin private key separately — only the creator holds this.
    let admin_key_path = circle_dir(&circle_id)?.join("admin.key");
    std::fs::write(&admin_key_path, &admin_privkey_hex)
        .map_err(|e| anyhow::anyhow!("failed to write admin.key: {e}"))?;

    // ── Generate invite ───────────────────────────────────────────────────────
    let ttl = invite::parse_ttl(&args.ttl)?;
    let invite_uri = invite::encode(&InvitePayload {
        circle_id:   circle_id.clone(),
        psk_bytes:   psk,
        circle_name: Some(args.name.clone()),
        expires_at:  Utc::now() + ttl,
        peer_addr:   None,
    });

    println!("✦ Circle cast: {}", args.name);
    println!("  circle-id : {circle_id}");
    println!("  peer-id   : {peer_id}");
    println!("  workspace : {}", workspace_dir.display());
    println!();
    println!("  invite    : {invite_uri}");
    println!();
    println!("  Share the invite link to let peers join (valid for {}).", args.ttl);
    println!("  Generate a new link anytime: enoch invite \"{}\"", args.name);

    Ok(())
}
