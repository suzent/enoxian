use std::io::Write;
use std::path::Path;

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
    // ── Validate arguments before anything is written ─────────────────────────
    // Everything below this point creates state: the workspace directory, the
    // circle config, the admin key, the MLS group. A TTL that cannot be turned
    // into an invite has to fail here, or the command exits with an error
    // having already made a circle.
    let ttl = invite::parse_ttl(&args.ttl)?;

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

    // The founder admits themselves; there is no invite to carry.
    let config = CircleConfig {
        circle_id: circle_id.clone(),
        circle_name: args.name.clone(),
        psk_hex: hex::encode(psk),
        keypair_proto_hex: keypair_to_hex(&keypair)?,
        join_grant: None,
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
    // The admin signing key: whoever reads it can mutate membership.
    config::write_secret(&admin_key_path, &admin_privkey_hex)
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

    // ── Seed the Circle's own conventions ─────────────────────────────────────
    // The circle is made by now, so a failure here is reported, not fatal.
    if let Err(e) = seed_agents_md(&workspace_dir, &args.name) {
        eprintln!("  warning: could not write AGENTS.md: {e}");
    }

    // ── Generate invite ───────────────────────────────────────────────────────
    let admin_pubkey_bytes = hex::decode(&admin_pubkey_hex).ok();
    let expires_at = Utc::now() + ttl;
    let grant = invite::sign_grant(&circle_id, &keypair_to_hex(&keypair)?, expires_at).ok();
    let invite_uri = invite::encode(&InvitePayload {
        circle_id: circle_id.clone(),
        psk_bytes: psk,
        circle_name: Some(args.name.clone()),
        expires_at,
        peer_addr: None,
        admin_pubkey_bytes,
        relay_addr: None,
        rendezvous_addr: None,
        // A brand-new circle has no saved servers to name, and the daemon is not
        // up yet to resolve one. `enox invite` embeds them once it is.
        relay_is_default: false,
        rendezvous_is_default: false,
        grant,
    })?;

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

/// A minimal root `AGENTS.md` for a new Circle, where members write down how
/// their shared folder is laid out. The standing agent brief tells every agent
/// to read it first (see `agent::context`).
///
/// Written once, when the Circle is created, and only if the folder has none:
/// `--dir` may point at a folder that already holds one. After that the file
/// belongs to the members, and enoxian never rewrites it. Devices that join
/// get it by sync like any other file.
fn seed_agents_md(workspace: &Path, circle_name: &str) -> std::io::Result<bool> {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(workspace.join("AGENTS.md"));
    let mut file = match file {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(e),
    };
    file.write_all(agents_md_template(circle_name).as_bytes())?;
    Ok(true)
}

fn agents_md_template(circle_name: &str) -> String {
    format!(
        "# {circle_name}\n\
         \n\
         This folder is the circle's shared knowledge base and index. Agents read this \
         file first; edit it to fit how the circle works.\n\
         \n\
         - `devices/<device>.md`: what each device is, what it runs, where its checkouts live\n\
         - `notes/`: shared working text and decisions\n\
         - `handoffs/`: work passed between devices or agents\n\
         \n\
         Keep repositories and build output out of this folder. Record where they live \
         instead (device, path, branch).\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_circle_gets_an_agents_md_naming_it() {
        let dir = tempfile::tempdir().unwrap();
        assert!(seed_agents_md(dir.path(), "Studio").unwrap());
        let text = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
        assert!(text.starts_with("# Studio\n\nThis folder is the circle's shared knowledge base"));
        assert!(text.contains("- `notes/`: shared working text and decisions\n"));
    }

    /// `--dir` can point at a folder that already has one. It is left alone.
    #[test]
    fn an_existing_agents_md_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("AGENTS.md");
        std::fs::write(&path, "ours\n").unwrap();
        assert!(!seed_agents_md(dir.path(), "Studio").unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "ours\n");
    }
}
