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
        IdentityAction::ForgetPhrase => forget_phrase(),
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
    // Installs made before the phrase stopped being written still have one on
    // disk, and it is the single worst thing there: with it, whoever finds the
    // machine is the user, on every device, permanently. Say so every time
    // rather than deleting it — the words may be the only copy.
    if device.stored_phrase().is_some() {
        println!();
        println!("  ⚠ Your recovery phrase is stored in this device's identity.toml.");
        println!("    Anyone who reads that file becomes you, on every device, for good.");
        println!("    Write the words down somewhere safe, then remove the stored copy:");
        println!("        enox identity forget-phrase");
        println!();
    }

    if !device.attestation_chain.is_empty() {
        // "Present" says nothing worth knowing. A linked device should be able
        // to show that the signatures it holds actually reach its own key.
        let hops = device.attestation_chain.len();
        if device.attestation_is_valid() {
            let via = match hops {
                1 => "signed by your user key".to_string(),
                n => format!("{n} hops from your user key"),
            };
            println!("  attestation: valid ({via})");
        } else {
            println!("  attestation: DOES NOT VERIFY — re-link this device");
        }
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
    user.link_device(&mut device)?;

    println!("✦ User identity created: {handle}");
    println!();
    println!("  ╔══════════════════════════════════════════════════════════════╗");
    println!("  ║  WRITE THESE WORDS DOWN NOW — they are shown once and are    ║");
    println!("  ║  not saved anywhere. They are the only way back if every     ║");
    println!("  ║  device you own is lost.                                     ║");
    println!("  ╚══════════════════════════════════════════════════════════════╝");
    println!();
    println!("  {mnemonic}");
    println!();
    println!("  Adding a device does NOT need them — run `enox link` from any");
    println!("  device already signed in. They are for the case where none is left:");
    println!("      enox identity link-user \"{handle}\" \"<the words>\"");
    println!();
    println!("  Run `enox service restart` for presence to reflect the change.");
    Ok(())
}

fn link_user(handle: String, mnemonic: String) -> Result<()> {
    let mut device =
        DeviceIdentity::load().context("no device identity — run `enox start` first")?;

    let user = UserIdentity::from_mnemonic(&mnemonic, handle.clone())?;
    user.link_device(&mut device)?;

    println!("✦ Device linked to user '{handle}'");
    println!("  Run `enox service restart` for presence to reflect the change.");
    Ok(())
}

/// Remove a recovery phrase left on disk by an older install.
///
/// Deliberately a separate step rather than something an upgrade does: the
/// stored words may be the only copy anyone has, and deleting them for someone
/// who never wrote them down would lose the identity outright the next time
/// every device is gone.
fn forget_phrase() -> Result<()> {
    let device = DeviceIdentity::load().context("no device identity — run `enox start` first")?;

    let Some(phrase) = device.stored_phrase() else {
        println!("✦ No recovery phrase is stored on this device — nothing to remove.");
        println!("  Newer installs never write one; it is shown once and only once.");
        return Ok(());
    };

    println!("  Last chance to copy these down — after this they are gone from here:");
    println!();
    println!("  {phrase}");
    println!();
    if !confirm("  Have you written them down somewhere safe?")? {
        println!("  Left as it was. Nothing was changed.");
        return Ok(());
    }

    device.forget_phrase()?;
    println!();
    println!("✦ Removed. This device no longer holds your recovery phrase.");
    println!("  It can still link new devices — `enox link` uses its attestation,");
    println!("  not the phrase.");
    Ok(())
}

/// Ask, and treat anything but an explicit yes as no.
fn confirm(question: &str) -> Result<bool> {
    use std::io::{BufRead, Write};
    print!("{question} [y/N] ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .context("reading your answer")?;
    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}
