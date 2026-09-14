//! `enox link` — put this identity on a second device.
//!
//! Run bare on the machine that already has the identity, and with the code it
//! prints on the new one:
//!
//! ```text
//!   $ enox link                      $ enox link atom cargo salsa civic
//!   Code: atom cargo salsa civic        Confirm on your other device: 731-508
//!   Confirm on the other device:     Does it show the same number? [y/N]
//!     731-508
//!   Does it match? [y/N]
//! ```
//!
//! Both sides ask, and neither acts until its own user has answered. See
//! [`crate::pairing`] for why that comparison is the security of the whole
//! thing, and what does and does not cross the wire.
//!
//! # What the new device ends up with
//!
//! Its own device key, which it generated locally and never sent; an
//! attestation over that key signed by the user root key, which never moves;
//! and one ordinary invite per circle. The invites are redeemed through exactly
//! the path a pasted invite takes, so admission stays the flow that already
//! exists — the joining device writes a pending entry with the grant in it, and
//! the circle admits it under its own join policy.

use anyhow::{bail, Context, Result};
use std::io::{BufRead, Write};
use std::time::{Duration, Instant};

use crate::{
    cli::LinkArgs,
    commands::rendezvous as rdvz,
    config,
    identity::DeviceIdentity,
    invite::{self, InvitePayload},
    pairing::{Code, LinkPayload, Offer, Session, SESSION_TIMEOUT_SECS, VERSION},
};

/// How long an invite minted for a linked device stays good.
///
/// Short: the device it is for is expected to redeem it within seconds, and a
/// link payload is the one place a pile of invites for every circle exists at
/// once. A day is enough slack for a machine that is paired now and started
/// later.
const INVITE_TTL: &str = "24h";

const POLL_INTERVAL: Duration = Duration::from_millis(700);

/// Most a mailbox slot can hold, mirroring the server's own cap with a little
/// slack. `--server` can name any host, so the client does not take the
/// server's word for how much it is about to be sent.
const MAX_REPLY: u64 = 80 * 1024;

pub async fn run(args: LinkArgs, _daemon_client: &reqwest::Client) -> Result<()> {
    // Deliberately not the caller's client. That one carries the local daemon's
    // bearer token as a default header, and every request below goes to a
    // bootstrap host over plain HTTP — handing a privileged local credential to
    // whoever runs it, before the user has confirmed anything.
    let client = &crate::outbound::client();

    let server = match args.server {
        Some(ref s) => s.clone(),
        None => crate::defaults::DEFAULT_RENDEZVOUS
            .context(
                "this build has no default pairing server — pass --server <host> \
                 pointing at an `enox bootstrap serve`",
            )?
            .to_string(),
    };
    let base = mailbox_base(&server);

    // Words arrive as separate arguments or as one quoted string; `Code::parse`
    // treats a space the same either way, so joining is all that is needed.
    if args.code.is_empty() {
        offer(&base, client).await
    } else {
        join(&args.code.join(" "), &base, client).await
    }
}

/// `http://<host>:<port>/pair` — the mailbox lives on the bootstrap server's
/// HTTP port, the same one `/peer-id` answers on.
fn mailbox_base(server: &str) -> String {
    let (host, port) = match server.rsplit_once(':') {
        Some((h, p)) if p.parse::<u16>().is_ok() => (h.to_string(), p.to_string()),
        _ => (server.to_string(), "36521".to_string()),
    };
    format!("http://{host}:{port}/pair")
}

// ── The device that already has the identity ─────────────────────────────────

async fn offer(base: &str, client: &reqwest::Client) -> Result<()> {
    let mut device = DeviceIdentity::load()
        .context("no identity on this device yet — run `enox start` once, then link from here")?;

    let code = Code::generate()?;
    let session = Session::new(code.clone())?;
    let mailbox = code.mailbox_id();

    // Publish this side's ephemeral key before showing the code, so that by the
    // time anyone is asked to confirm, both machines can already display the
    // number. Confirming on one screen while the other is still blank would
    // make the comparison meaningless.
    put(
        client,
        &format!("{base}/{mailbox}/hello"),
        session.ephemeral_pubkey_bytes(),
    )
    .await?;

    println!("✦ Linking a new device");
    println!();
    println!("  On the other machine, run:");
    println!();
    println!("      enox link {}", code.display());
    println!();
    println!("  Waiting for it to connect (the code is good for 2 minutes)…");

    let sealed_offer = poll(client, &format!("{base}/{mailbox}/offer")).await?;
    let offer = code.open_offer(&sealed_offer)?;
    let agreed = session.agree(&offer.ephemeral_pubkey, true)?;

    println!();
    println!("  The other device says it is '{}'.", offer.device_label);
    println!();
    println!(
        "      Confirmation number: {}",
        agreed.confirmation_number()
    );
    println!();
    if !confirm("  Does the other device show the same number?")? {
        bail!("pairing cancelled — nothing was sent");
    }

    // Only past the confirmation is anything built, let alone sent.
    let payload = build_payload(&mut device, &offer, &agreed.transcript_hash())?;
    let circles = payload.invites.len();
    let sealed = agreed.seal_payload(&payload)?;

    put(client, &format!("{base}/{mailbox}/reply"), sealed).await?;

    println!();
    println!("✦ Sent to '{}'.", offer.device_label);
    println!(
        "  {circles} circle{} passed over. Confirm on the other device to finish.",
        if circles == 1 { "" } else { "s" }
    );
    if payload.user_attestation_hex.is_none() && payload.user_handle.is_some() {
        println!();
        println!("  Note: this device does not hold the user root key, so the new device");
        println!("  joins the circles but is not attested to your user identity. Link it");
        println!("  from the device you ran `enox identity create-user` on to do that.");
    }
    Ok(())
}

/// Assemble what the new device needs: an attestation over the key it made for
/// itself, and an invite per circle.
fn build_payload(
    device: &mut DeviceIdentity,
    offer: &Offer,
    transcript_hash: &str,
) -> Result<LinkPayload> {
    // The root key is only on the device it was created on. Without it this
    // device can still hand over its circles — it just cannot vouch for the new
    // device's key, which `offer()` says out loud rather than leaving to be
    // discovered later.
    let user = device.user_identity()?;
    // `attest_device` refuses a device key that is not a decodable libp2p key,
    // so a target cannot use this step to have the root key sign bytes of its
    // own choosing.
    let attestation = match user {
        Some(ref u) => Some(u.attest_device(&offer.device_pubkey_hex)?),
        None => None,
    };
    let user_pubkey_hex = match user {
        Some(ref u) => Some(u.pubkey_hex()?),
        None => None,
    };

    let mut invites = Vec::new();
    for circle in config::load_all()? {
        if circle.disabled {
            continue;
        }
        match mint_invite(&circle) {
            Ok(uri) => invites.push(uri),
            // One unreadable circle config should not cost the user every other
            // circle on the machine — say which, and carry on.
            Err(e) => {
                println!("  (skipping '{}': {e})", circle.circle_name);
            }
        }
    }

    Ok(LinkPayload {
        version: VERSION,
        transcript_hash: transcript_hash.to_string(),
        user_handle: device.user_handle.clone(),
        user_pubkey_hex,
        user_attestation_hex: attestation,
        invites,
    })
}

fn mint_invite(circle: &config::CircleConfig) -> Result<String> {
    let psk: [u8; 32] = hex::decode(&circle.psk_hex)
        .context("config.toml has invalid psk_hex")?
        .try_into()
        .map_err(|_| anyhow::anyhow!("psk_hex must be 32 bytes"))?;

    let expires_at = chrono::Utc::now() + invite::parse_ttl(INVITE_TTL)?;
    let grant = invite::sign_grant(&circle.circle_id, &circle.keypair_proto_hex, expires_at).ok();

    let relay_addr = circle.relay_addrs.first().cloned();
    let rendezvous_addr = circle.rendezvous_addrs.first().cloned();
    let relay_is_default = relay_addr.as_deref().is_some_and(rdvz::is_default_relay);
    let rendezvous_is_default = rendezvous_addr
        .as_deref()
        .is_some_and(rdvz::is_default_rendezvous);

    invite::encode(&InvitePayload {
        circle_id: circle.circle_id.clone(),
        psk_bytes: psk,
        circle_name: Some(circle.circle_name.clone()),
        expires_at,
        // No peer address: this side may not have a daemon up, and the new
        // device reaches the circle the same way this one does — through the
        // relay and rendezvous server the config already names.
        peer_addr: None,
        admin_pubkey_bytes: hex::decode(&circle.admin_pubkey_hex)
            .ok()
            .filter(|b| !b.is_empty()),
        relay_addr: relay_addr.filter(|_| !relay_is_default),
        rendezvous_addr: rendezvous_addr.filter(|_| !rendezvous_is_default),
        relay_is_default,
        rendezvous_is_default,
        grant,
    })
}

// ── The new device ────────────────────────────────────────────────────────────

async fn join(code_input: &str, base: &str, client: &reqwest::Client) -> Result<()> {
    let code = Code::parse(code_input)?;
    let mailbox = code.mailbox_id();

    // The device key is generated here and stays here. If this machine already
    // has one, it keeps it — linking adds an identity, it does not replace the
    // device.
    let mut device = DeviceIdentity::load_or_generate(None)?;
    let device_pubkey_hex = hex::encode(device.device_keypair()?.public().encode_protobuf());

    let session = Session::new(code.clone())?;
    let offer = Offer {
        version: VERSION,
        session_id: code.session_id_hex(),
        ephemeral_pubkey: session.ephemeral_pubkey_hex(),
        device_pubkey_hex,
        device_label: device.device_label.clone(),
    };

    println!("✦ Linking this device");
    println!("  Device: {}", device.device_label);
    println!();

    // The source's ephemeral key is not in the code — a code short enough to
    // retype has no room for 32 bytes — so it is fetched from the mailbox.
    let source_pubkey = poll(client, &format!("{base}/{mailbox}/hello")).await?;
    let agreed = session.agree(&hex::encode(&source_pubkey), false)?;

    put(
        client,
        &format!("{base}/{mailbox}/offer"),
        code.seal_offer(&offer)?,
    )
    .await?;

    println!("  Confirmation number: {}", agreed.confirmation_number());
    println!();
    if !confirm("  Does your other device show the same number?")? {
        bail!("pairing cancelled — nothing was applied");
    }

    println!("  Waiting for the other device to confirm…");
    let sealed_payload = poll(client, &format!("{base}/{mailbox}/reply")).await?;

    // Held as sealed bytes until both sides have said yes. The transcript is
    // checked inside `open_payload`, so a payload from a session the source did
    // not confirm cannot reach this code at all.
    let payload = agreed.open_payload(&sealed_payload)?;

    // A device that already belongs to one user must not be quietly moved to
    // another. Refuse before writing anything; re-linking to the same identity
    // is fine and re-attests.
    if let (Some(existing), Some(incoming)) = (
        device.claimed_user_pubkey(),
        payload.user_pubkey_hex.as_deref(),
    ) {
        if existing != incoming {
            bail!(
                "this device already belongs to user '{}' — linking it to a different \
                 identity would strand it. Run `enox identity show` on both devices, and \
                 remove this one from the old identity first if that is what you want.",
                device.user_handle.as_deref().unwrap_or("(unknown)")
            );
        }
    }

    if let (Some(handle), Some(pubkey), Some(attestation)) = (
        payload.user_handle.clone(),
        payload.user_pubkey_hex.clone(),
        payload.user_attestation_hex.clone(),
    ) {
        // Checked against this device's own key inside `adopt_attestation`, so
        // an attestation that proves nothing is never written to disk.
        device.adopt_attestation(handle.clone(), pubkey, attestation)?;
        device.save()?;
        println!("  Linked to user '{handle}'.");
    } else if let Some(handle) = payload.user_handle.clone() {
        device.set_user_handle(handle.clone());
        device.save()?;
        println!("  Handle set to '{handle}' (not attested — see the other device).");
    }

    let mut joined = 0usize;
    for uri in &payload.invites {
        match enter_circle(uri, client).await {
            Ok(name) => {
                joined += 1;
                println!("  Joined '{name}'.");
            }
            Err(e) => println!("  Could not join one circle: {e}"),
        }
    }

    println!();
    println!(
        "✦ This device is linked. {joined} circle{} ready.",
        if joined == 1 { "" } else { "s" }
    );
    println!("  Run `enox start` to connect.");
    Ok(())
}

async fn enter_circle(uri: &str, client: &reqwest::Client) -> Result<String> {
    let payload = invite::decode(uri)?;
    let name = payload
        .circle_name
        .clone()
        .unwrap_or_else(|| payload.circle_id.clone());

    crate::commands::enter::run(
        crate::cli::EnterArgs {
            target: uri.to_string(),
            secret: None,
            dir: None,
            peer: None,
            rendezvous: None,
            owner: None,
            no_verify: true,
        },
        client,
    )
    .await?;
    Ok(name)
}

// ── Shared plumbing ───────────────────────────────────────────────────────────

/// Write a slot, turning the mailbox's refusals into something a user can act on.
async fn put(client: &reqwest::Client, url: &str, body: Vec<u8>) -> Result<()> {
    let status = client
        .post(url)
        .body(body)
        .send()
        .await
        .context("could not reach the pairing server")?
        .status();
    match status {
        s if s.is_success() => Ok(()),
        reqwest::StatusCode::CONFLICT => bail!(
            "that code is already in use by another device — \
             start `enox link` again for a fresh one"
        ),
        reqwest::StatusCode::SERVICE_UNAVAILABLE => {
            bail!("the pairing server is busy — try again in a moment")
        }
        other => bail!("the pairing server refused the message ({other})"),
    }
}

/// Poll a slot until the other side writes to it, or the session window closes.
async fn poll(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let deadline = Instant::now() + Duration::from_secs(SESSION_TIMEOUT_SECS);
    loop {
        let resp = client
            .get(url)
            .send()
            .await
            .context("could not reach the pairing server")?;
        if resp.status().is_success() {
            // A slot is capped at `MAX_SLOT` server-side, but `--server` can
            // name any host, so the cap is enforced here too rather than
            // buffering whatever arrives.
            if let Some(len) = resp.content_length() {
                if len > MAX_REPLY {
                    bail!("the pairing server returned an implausibly large reply");
                }
            }
            let body = resp.bytes().await.context("reading the pairing reply")?;
            if body.len() as u64 > MAX_REPLY {
                bail!("the pairing server returned an implausibly large reply");
            }
            return Ok(body.to_vec());
        }
        if Instant::now() >= deadline {
            bail!(
                "nothing arrived within {SESSION_TIMEOUT_SECS} seconds — \
                 the code has expired, start again"
            );
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Ask, and treat anything but an explicit yes as no.
///
/// The prompt is the last thing standing between a man in the middle and the
/// payload, so a stray newline must not be able to answer it.
fn confirm(question: &str) -> Result<bool> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_host_gets_the_default_pairing_port() {
        assert_eq!(
            mailbox_base("relay.enoxian.com"),
            "http://relay.enoxian.com:36521/pair"
        );
    }

    #[test]
    fn an_explicit_port_is_kept() {
        assert_eq!(mailbox_base("localhost:9999"), "http://localhost:9999/pair");
    }

    /// A host that merely contains a colon-ish tail must not be mistaken for
    /// one carrying a port.
    #[test]
    fn a_non_numeric_tail_is_not_a_port() {
        assert_eq!(
            mailbox_base("relay.enoxian.com:pair"),
            "http://relay.enoxian.com:pair:36521/pair"
        );
    }
}
