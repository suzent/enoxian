use anyhow::Result;
use uuid::Uuid;

use crate::{
    cli::InitArgs,
    config::{save, CircleConfig},
    crypto::{generate_keypair, generate_psk, keypair_to_hex},
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

    println!("✦ Circle cast: {}", args.name);
    println!("  circle-id : {circle_id}");
    println!("  peer-id   : {peer_id}");
    println!("  secret    : {}", hex::encode(psk));
    println!();
    println!("Config saved to ~/.enochian/circles/{circle_id}/config.toml");
    println!("Share the circle-id and secret with peers to let them enter.");

    Ok(())
}
