use crate::{
    cli::{IdentityAction, IdentityArgs},
    identity::{DeviceIdentity, UserIdentity},
};
use anyhow::{Context, Result};

pub fn run(args: IdentityArgs) -> Result<()> {
    match args.action {
        IdentityAction::Show => show(),
        IdentityAction::SetLabel { label } => set_label(label),
        IdentityAction::SetUser { handle } => set_user_handle(handle),
        IdentityAction::CreateUser { handle } => create_user(handle),
        IdentityAction::LinkUser { handle, mnemonic } => link_user(handle, mnemonic),
    }
}

fn show() -> Result<()> {
    let device = DeviceIdentity::load()
        .context("no device identity found — run `enox start` once to create one")?;
    let kp = device.device_keypair()?;
    let peer_id = kp.public().to_peer_id();
    println!("Device identity");
    println!("  label      : {}", device.device_label);
    println!("  peer ID    : {peer_id}");
    if let Some(ref handle) = device.user_handle {
        println!("  user       : {handle}");
    } else {
        println!("  user       : (none — run `enox identity create-user <handle>`)");
    }
    if let Some(ref pk) = device.user_pubkey_hex {
        println!("  user pubkey: {pk}");
    }
    if device.user_attestation_hex.is_some() {
        println!("  attestation: present");
    }
    Ok(())
}

fn set_label(label: String) -> Result<()> {
    let mut device =
        DeviceIdentity::load().context("no device identity — run `enox start` first")?;
    device.device_label = label.clone();
    device.save()?;
    println!("Device label updated to '{label}'");
    println!("Run `enox service restart` for presence to reflect the change.");
    Ok(())
}

fn set_user_handle(handle: String) -> Result<()> {
    let mut device =
        DeviceIdentity::load().context("no device identity — run `enox start` first")?;
    device.set_user_handle(handle.clone());
    device.save()?;
    println!("User handle set to '{handle}'");
    println!("Run `enox service restart` for presence to reflect the change.");
    Ok(())
}

fn create_user(handle: String) -> Result<()> {
    let mut device =
        DeviceIdentity::load().context("no device identity — run `enox start` first")?;

    let (user, mnemonic) = UserIdentity::generate(handle.clone())?;
    user.link_device(&mut device, &mnemonic)?;

    println!("✦ User identity created: {handle}");
    println!();
    println!("  ╔══════════════════════════════════════════════════════════════╗");
    println!("  ║  BACKUP YOUR MNEMONIC — write these words down now.          ║");
    println!("  ║  You will need them to link other devices to this user.      ║");
    println!("  ╚══════════════════════════════════════════════════════════════╝");
    println!();
    println!("  {mnemonic}");
    println!();
    println!("  To link another device: enox identity link-user \"{handle}\" \"<mnemonic>\"");
    println!("  Run `enox service restart` for presence to reflect the change.");
    Ok(())
}

fn link_user(handle: String, mnemonic: String) -> Result<()> {
    let mut device =
        DeviceIdentity::load().context("no device identity — run `enox start` first")?;

    let user = UserIdentity::from_mnemonic(&mnemonic, handle.clone())?;
    user.link_device(&mut device, &mnemonic)?;

    println!("✦ Device linked to user '{handle}'");
    println!("  Run `enox service restart` for presence to reflect the change.");
    Ok(())
}
