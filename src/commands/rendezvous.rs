use anyhow::{bail, Context, Result};

/// Resolve a rendezvous server address into a full libp2p multiaddr.
///
/// Accepts:
///   - A full multiaddr: `/ip4/1.2.3.4/udp/36521/quic-v1/p2p/<id>` — returned as-is
///   - A hostname or IP with optional port: `enox.suzent.com`, `enox.suzent.com:4001`,
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

/// Resolve the default rendezvous server defined in `crate::defaults::DEFAULT_RENDEZVOUS`.
/// Returns `None` if the constant is unset or the server cannot be reached (non-fatal).
pub async fn resolve_default() -> Option<String> {
    let host = crate::defaults::DEFAULT_RENDEZVOUS?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()?;
    resolve(host, &client).await.ok()
}

/// Resolve the default relay server defined in `crate::defaults::DEFAULT_RELAY`.
/// Returns `None` if the constant is unset or the server cannot be reached (non-fatal).
///
/// If `DEFAULT_RELAY` and `DEFAULT_RENDEZVOUS` point to the same host the result
/// is identical — we reuse the same `/peer-id` fetch so both share the same
/// resolved multiaddr.
pub async fn resolve_default_relay() -> Option<String> {
    let host = crate::defaults::DEFAULT_RELAY?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()?;
    resolve_relay(host, &client).await.ok()
}

/// Resolve a bootstrap relay address into a TCP libp2p relay multiaddr.
///
/// Short host forms use the same HTTP `/peer-id` endpoint as rendezvous
/// resolution, but relay traffic itself runs on TCP port `http_port + 1` by
/// default so it does not collide with the HTTP control endpoint.
pub async fn resolve_relay(input: &str, client: &reqwest::Client) -> Result<String> {
    if input.starts_with('/') {
        return Ok(input.to_string());
    }

    let (host, http_port) = split_host_port(input, 36521);
    let relay_port = http_port.saturating_add(1);
    let url = format!("http://{host}:{http_port}/peer-id");
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

    if host.parse::<std::net::Ipv4Addr>().is_ok() {
        Ok(format!("/ip4/{host}/tcp/{relay_port}/p2p/{peer_id}"))
    } else {
        Ok(format!("/dns4/{host}/tcp/{relay_port}/p2p/{peer_id}"))
    }
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
