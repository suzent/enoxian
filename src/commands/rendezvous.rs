use anyhow::{bail, Context, Result};

/// Resolve a rendezvous server address into a full libp2p multiaddr.
///
/// Accepts:
///   - A full multiaddr: `/ip4/1.2.3.4/udp/36521/quic-v1/p2p/<id>` — returned as-is
///   - A hostname or IP with optional port: `enoch.suzent.com`, `enoch.suzent.com:4001`,
///     `1.2.3.4`, `1.2.3.4:4001`
///
/// For the short forms, the CLI fetches `GET http://<host>:<port>/peer-id` from the
/// bootstrap server's built-in HTTP endpoint, then constructs the full multiaddr.
/// Default port: 36521.
pub async fn resolve(input: &str, client: &reqwest::Client) -> Result<String> {
    if input.starts_with('/') {
        return Ok(input.to_string());
    }

    let (host, port) = split_host_port(input, 36521);

    let url = format!("http://{host}:{port}/peer-id");
    let resp = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .with_context(|| format!("could not reach bootstrap server at {url} — is it running?"))?;

    if !resp.status().is_success() {
        bail!("bootstrap server at {url} returned {}", resp.status());
    }

    let json: serde_json::Value = resp.json().await
        .context("bootstrap server returned invalid JSON")?;
    let peer_id = json["peer_id"].as_str()
        .context("bootstrap server response missing 'peer_id' field")?;

    // Use /dns4/ for hostnames so the address stays valid if the IP changes.
    // Use /ip4/ for bare IP addresses.
    let multiaddr = if host.parse::<std::net::Ipv4Addr>().is_ok() {
        format!("/ip4/{host}/udp/{port}/quic-v1/p2p/{peer_id}")
    } else {
        format!("/dns4/{host}/udp/{port}/quic-v1/p2p/{peer_id}")
    };

    Ok(multiaddr)
}

fn split_host_port(input: &str, default_port: u16) -> (String, u16) {
    // Handle host:port
    if let Some(colon) = input.rfind(':') {
        let maybe_port = &input[colon + 1..];
        if let Ok(p) = maybe_port.parse::<u16>() {
            return (input[..colon].to_string(), p);
        }
    }
    (input.to_string(), default_port)
}
