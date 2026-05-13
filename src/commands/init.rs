use anyhow::Result;
use chrono::Utc;
use uuid::Uuid;

use crate::{
    cli::InitArgs,
    config::{save, CircleConfig},
    crypto::{generate_keypair, generate_psk, keypair_to_hex},
    invite::{self, InvitePayload},
};

pub async fn run(args: InitArgs) -> Result<()> {
    let circle_id = Uuid::new_v4().to_string();
    let psk = generate_psk();
    let keypair = generate_keypair();
    let peer_id = keypair.public().to_peer_id();

    let config = CircleConfig {
        circle_id: circle_id.clone(),
        circle_name: args.name.clone(),
        psk_hex: hex::encode(psk),
        keypair_proto_hex: keypair_to_hex(&keypair)?,
    };

    save(&config)?;

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
    println!();
    println!("  invite    : {invite_uri}");
    println!();
    println!("  Share the invite link to let peers join (valid for {}).", args.ttl);
    println!("  Generate a new link anytime: enoch invite {circle_id}");
    println!();
    println!("Config saved to ~/.enochian/circles/{circle_id}/config.toml");

    Ok(())
}
