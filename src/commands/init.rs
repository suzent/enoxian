use anyhow::{bail, Result};
use chrono::Utc;
use uuid::Uuid;

use crate::{
    cli::InitArgs,
    config::{self, circle_dir, default_workspace_dir, CircleConfig},
    crypto::{generate_keypair, generate_psk, keypair_to_hex},
    identity::DeviceIdentity,
    invite::{self, InvitePayload},
    mls::{MlsGroupManager, MlsIdentity},
};

pub async fn run(args: InitArgs) -> Result<()> {
    // ── Enforce unique name locally ───────────────────────────────────────────
    let existing = config::load_all()?;
    if existing.iter().any(|c| c.circle_name == args.name) {
        bail!(
            "a circle named '{}' already exists — run `enox circles` to list existing circles, or choose a different name",
            args.name
        );
    }

    // ── Resolve workspace directory ───────────────────────────────────────────
    let circle_id = Uuid::new_v4().to_string();
    let workspace_dir = match args.dir {
        Some(d) => {
            let d = config::normalize_workspace_dir(&d)?;
            if let Some(conflict) = config::workspace_conflict(&d, &circle_id, &existing)? {
                bail!(
                    "workspace {} is already owned by circle '{}' ({})",
                    d.display(),
                    conflict.circle_name,
                    conflict.circle_id
                );
            }
            d
        }
        None => {
            let default = config::normalize_workspace_dir(&default_workspace_dir(&args.name)?)?;
            if config::workspace_conflict(&default, &circle_id, &existing)?.is_some() {
                config::normalize_workspace_dir(&config::disambiguated_workspace_dir(
                    &args.name, &circle_id,
                )?)?
            } else {
                default
            }
        }
    };
    tokio::fs::create_dir_all(&workspace_dir).await?;
    let workspace_dir = config::normalize_workspace_dir(&workspace_dir)?;
    if let Some(conflict) = config::workspace_conflict(&workspace_dir, &circle_id, &existing)? {
        bail!(
            "workspace {} resolves to a directory already owned by circle '{}' ({})",
            workspace_dir.display(),
            conflict.circle_name,
            conflict.circle_id
        );
    }

    // ── Generate credentials ──────────────────────────────────────────────────
    let psk = generate_psk();
    // Use the stable device identity to derive a per-circle keypair so this
    // device always presents the same peer ID in this circle across restarts.
    let device = DeviceIdentity::load_or_generate(None)?;
    let keypair = device.derive_circle_keypair(&circle_id)?;
    let peer_id = keypair.public().to_peer_id();

    // Admin keypair — generated now; enforcement added in M6.
    // Private key lives only on this machine; public key is shared in config.
    let admin_keypair = generate_keypair();
    let admin_pubkey_hex = hex::encode(admin_keypair.public().encode_protobuf());
    let admin_privkey_hex = keypair_to_hex(&admin_keypair)?;

    let peer_id_str = peer_id.to_string();
    let owner = args.owner.unwrap_or_else(|| {
        crate::identity::read_identity_display()
            .map(|(label, handle)| handle.unwrap_or(label))
            .unwrap_or_else(|| peer_id_str.clone())
    });
    let join_policy = match args.join_policy.to_lowercase().as_str() {
        "manual" => crate::config::JoinPolicy::Manual,
        _ => crate::config::JoinPolicy::Auto,
    };

    let config = CircleConfig {
        circle_id: circle_id.clone(),
        circle_name: args.name.clone(),
        psk_hex: hex::encode(psk),
        keypair_proto_hex: keypair_to_hex(&keypair)?,
        workspace_dir: workspace_dir.to_string_lossy().into_owned(),
        admin_pubkey_hex: admin_pubkey_hex.clone(),
        disabled: false,
        force_relay: false,
        peers: vec![],
        relay_addrs: vec![],
        rendezvous_addrs: vec![],
        join_policy,
        owner,
    };
    config::save(&config)?;

    // Save admin private key separately — only the creator holds this.
    let admin_key_path = circle_dir(&circle_id)?.join("admin.key");
    std::fs::write(&admin_key_path, &admin_privkey_hex)
        .map_err(|e| anyhow::anyhow!("failed to write admin.key: {e}"))?;

    // ── Bootstrap MLS group (M11) ─────────────────────────────────────────────
    // Creator starts a single-member MLS group. Other members join via Welcome
    // messages distributed through the control doc (mls_welcomes).
    let cdir = circle_dir(&circle_id)?;
    let mls_identity = MlsIdentity::generate(&peer_id.to_string())?;
    mls_identity.save(&cdir)?;
    let mls_group = MlsGroupManager::create(&mls_identity)?;
    mls_group
        .save(&mls_identity, &cdir)
        .map_err(|e| anyhow::anyhow!("failed to save MLS group: {e}"))?;

    // ── Generate invite ───────────────────────────────────────────────────────
    let ttl = invite::parse_ttl(&args.ttl)?;
    let admin_pubkey_bytes = hex::decode(&admin_pubkey_hex).ok();
    let invite_uri = invite::encode(&InvitePayload {
        circle_id: circle_id.clone(),
        psk_bytes: psk,
        circle_name: Some(args.name.clone()),
        expires_at: Utc::now() + ttl,
        peer_addr: None,
        admin_pubkey_bytes,
        relay_addr: None,
        rendezvous_addr: None,
    });

    println!("✦ Circle cast: {}", args.name);
    println!("  circle-id : {circle_id}");
    println!("  peer-id   : {peer_id}");
    println!("  workspace : {}", workspace_dir.display());
    println!();
    println!("  invite    : {invite_uri}");
    println!();
    println!(
        "  Share the invite link to let peers join (valid for {}).",
        args.ttl
    );
    println!(
        "  Generate a new link anytime: enox invite \"{}\"",
        args.name
    );

    Ok(())
}
