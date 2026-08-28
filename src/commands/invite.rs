use anyhow::{Context, Result};
use chrono::Utc;

use crate::{
    cli::InviteArgs,
    commands::rendezvous as rdvz,
    config::{circle_dir, load_all},
    crypto::keypair_from_hex,
    invite::{self, InvitePayload},
    resolve,
};

pub async fn run(args: InviteArgs, client: &reqwest::Client, api_base: &str) -> Result<()> {
    let configs = load_all()?;
    let config = resolve::resolve(&args.circle, &configs)
        .with_context(|| {
            format!(
                "circle '{}' not found — run `enox circles` to list known circles",
                args.circle
            )
        })?
        .clone();

    let psk_bytes = hex::decode(&config.psk_hex).context("config.toml has invalid psk_hex")?;
    let psk: [u8; 32] = psk_bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("psk_hex must be 32 bytes (64 hex chars)"))?;

    let ttl = invite::parse_ttl(&args.ttl)?;
    let expires_at = Utc::now() + ttl;

    // ── Query daemon for live P2P info (optional — works even if daemon is down) ──
    let p2p = fetch_p2p_info(client, api_base).await;

    // ── Auto-detect addresses — explicit CLI flags always override ─────────────
    // peer_addr priority:
    //   1. explicit --peer flag
    //   2. ExternalAddrConfirmed (Identify from a connected peer — most reliable)
    //   3. best listen addr sorted: public IP > Tailscale (100.64/10) > RFC1918
    let peer_addr = args
        .peer
        .clone()
        .or_else(|| p2p.as_ref()?.external_addrs.first().cloned())
        .or_else(|| {
            let addrs = p2p.as_ref()?.listen_addrs.as_slice();
            best_listen_addr(addrs).map(String::from)
        });

    // relay_addr: from circle config (saved at `enox enter` time from the invite).
    // The user never has to think about this — if they joined via a relay invite,
    // they can forward that same relay to the people they invite.
    let cli_relay = match args.relay {
        Some(ref s) => Some(
            rdvz::resolve_relay(s, client)
                .await
                .with_context(|| format!("could not resolve relay server '{s}'"))?,
        ),
        None => None,
    };
    let relay_addr = if let Some(addr) = cli_relay.or_else(|| config.relay_addrs.first().cloned()) {
        Some(addr)
    } else {
        rdvz::resolve_default_relay().await
    };

    // rendezvous_addr: explicit flag (auto-resolved) > saved in circle config.
    let cli_rendezvous = match args.rendezvous {
        Some(ref s) => Some(
            rdvz::resolve(s, client)
                .await
                .with_context(|| format!("could not resolve rendezvous server '{s}'"))?,
        ),
        None => None,
    };
    let rendezvous_addr = cli_rendezvous.or_else(|| config.rendezvous_addrs.first().cloned());

    // Embed admin pubkey if admin.key is present (only on admin machines)
    let admin_pubkey_bytes = try_load_admin_pubkey(&config.circle_id);

    // Signed with this member's own circle key — any member can invite; the
    // grant records which of them did, so the invite can be checked against
    // their standing when it is redeemed.
    let grant = invite::sign_grant(&config.circle_id, &config.keypair_proto_hex, expires_at).ok();
    let uri = invite::encode(&InvitePayload {
        circle_id: config.circle_id.clone(),
        psk_bytes: psk,
        circle_name: Some(config.circle_name.clone()),
        expires_at,
        peer_addr: peer_addr.clone(),
        admin_pubkey_bytes,
        relay_addr: relay_addr.clone(),
        rendezvous_addr: rendezvous_addr.clone(),
        grant,
    });

    println!(
        "✦ Invite for '{}' (valid {}):",
        config.circle_name, args.ttl
    );
    println!();
    println!("  {uri}");
    println!();

    // Show the user what was auto-embedded so they're not surprised.
    println!("  Embedded connectivity:");
    if let Some(ref info) = p2p {
        println!("    peer-id   : {}", info.peer_id);
    }
    if let Some(ref a) = peer_addr {
        println!("    peer      : {a}");
    }
    if let Some(ref a) = relay_addr {
        println!("    relay     : {a}");
    }
    if let Some(ref a) = rendezvous_addr {
        println!("    rendezvous: {a}");
    }
    if peer_addr.is_none() && relay_addr.is_none() && rendezvous_addr.is_none() {
        println!("    (none — joinees will connect via mDNS on the same LAN)");
        if p2p.is_none() {
            println!("    Tip: start the daemon first for auto-detected WAN addresses.");
        }
    }
    println!();
    println!("  Join with: enox enter \"<invite>\"");

    Ok(())
}

struct P2PInfo {
    peer_id: String,
    external_addrs: Vec<String>,
    listen_addrs: Vec<String>,
}

async fn fetch_p2p_info(client: &reqwest::Client, api_base: &str) -> Option<P2PInfo> {
    let resp = client
        .get(format!("{api_base}/status"))
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await
        .ok()?;
    let json: serde_json::Value = resp.json().await.ok()?;
    let p2p = json.get("p2p")?;
    let peer_id = p2p.get("peer_id")?.as_str()?.to_string();
    let parse_addrs = |key: &str| -> Vec<String> {
        p2p.get(key)
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    Some(P2PInfo {
        peer_id,
        external_addrs: parse_addrs("external_addrs"),
        listen_addrs: parse_addrs("listen_addrs"),
    })
}

/// Pick the best listen addr from the list: public IPs first, then Tailscale
/// (100.64/10), then RFC1918. Returns the highest-priority addr, or None if empty.
fn best_listen_addr(addrs: &[String]) -> Option<&str> {
    fn rank(addr: &str) -> u8 {
        // Parse IPv4 from a multiaddr string like /ip4/1.2.3.4/tcp/...
        let ip_str = match addr.strip_prefix("/ip4/").and_then(|s| s.split('/').next()) {
            Some(s) => s,
            None => return 4,
        };
        let ip: std::net::Ipv4Addr = match ip_str.parse() {
            Ok(ip) => ip,
            Err(_) => return 4,
        };
        if ip.is_private() || ip.is_link_local() {
            return 3; // RFC1918 / link-local — least preferred
        }
        let o = ip.octets();
        if o[0] == 100 && o[1] >= 64 && o[1] <= 127 {
            return 2; // Tailscale CGNAT — usable within a tailnet
        }
        1 // public IP — most preferred
    }
    addrs
        .iter()
        .min_by_key(|a| rank(a.as_str()))
        .map(String::as_str)
}

fn try_load_admin_pubkey(circle_id: &str) -> Option<Vec<u8>> {
    let key_path = circle_dir(circle_id).ok()?.join("admin.key");
    let hex = std::fs::read_to_string(&key_path).ok()?;
    let keypair = keypair_from_hex(hex.trim()).ok()?;
    Some(keypair.public().encode_protobuf())
}
