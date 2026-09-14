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
pub async fn resolve(input: &str, _daemon_client: &reqwest::Client) -> Result<String> {
    if input.starts_with('/') {
        return Ok(input.to_string());
    }
    // Not the caller's client: it carries the local daemon's bearer token as a
    // default header, and `<host>/peer-id` is somebody else's server.
    let client = &crate::outbound::client();

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

    let json: serde_json::Value = resp
        .json()
        .await
        .context("bootstrap server returned invalid JSON")?;
    let peer_id = json["peer_id"]
        .as_str()
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
pub async fn resolve_relay(input: &str, _daemon_client: &reqwest::Client) -> Result<String> {
    if input.starts_with('/') {
        return Ok(input.to_string());
    }
    // As `resolve`: a bootstrap host must not be handed daemon credentials.
    let client = &crate::outbound::client();

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

    let json: serde_json::Value = resp
        .json()
        .await
        .context("bootstrap server returned invalid JSON")?;
    let peer_id = json["peer_id"]
        .as_str()
        .context("bootstrap server response missing 'peer_id' field")?;

    if host.parse::<std::net::Ipv4Addr>().is_ok() {
        Ok(format!("/ip4/{host}/tcp/{relay_port}/p2p/{peer_id}"))
    } else {
        Ok(format!("/dns4/{host}/tcp/{relay_port}/p2p/{peer_id}"))
    }
}

/// Whether `addr` is the address `resolve_relay` would produce for the
/// compiled-in [`crate::defaults::DEFAULT_RELAY`].
///
/// Judged offline, by host and port, so minting an invite does not have to ask
/// the bootstrap server just to discover it is about to embed the default. The
/// peer ID is deliberately not compared: it is the one part of the address the
/// joiner will fetch fresh anyway, and a server that has rotated its key would
/// otherwise make every invite grow by an address that is already stale.
///
/// A wrong `false` only costs bytes — the address is embedded verbatim, which
/// is what v1 always did.
pub fn is_default_relay(addr: &str) -> bool {
    let Some(host) = crate::defaults::DEFAULT_RELAY else {
        return false;
    };
    let (host, http_port) = split_host_port(host, 36521);
    matches_prefix(addr, &host, "tcp", http_port.saturating_add(1), "")
}

/// As [`is_default_relay`], for [`crate::defaults::DEFAULT_RENDEZVOUS`].
pub fn is_default_rendezvous(addr: &str) -> bool {
    let Some(host) = crate::defaults::DEFAULT_RENDEZVOUS else {
        return false;
    };
    let (host, port) = split_host_port(host, 36521);
    matches_prefix(addr, &host, "udp", port, "/quic-v1")
}

/// Whether `addr` starts with the host/transport/port that resolving `host`
/// would have produced, using the same `/ip4` vs `/dns4` choice as `resolve`.
fn matches_prefix(addr: &str, host: &str, transport: &str, port: u16, suffix: &str) -> bool {
    let scheme = if host.parse::<std::net::Ipv4Addr>().is_ok() {
        "ip4"
    } else {
        "dns4"
    };
    let prefix = format!("/{scheme}/{host}/{transport}/{port}{suffix}/");
    addr.starts_with(&prefix)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The address the default relay resolves to must be recognised as the
    /// default, or every invite would carry it verbatim and v2's saving would
    /// quietly disappear.
    #[test]
    fn the_resolved_default_relay_is_recognised() {
        let Some(host) = crate::defaults::DEFAULT_RELAY else {
            return; // a build with no default has nothing to elide
        };
        let addr = format!("/dns4/{host}/tcp/36522/p2p/12D3KooWanything");
        assert!(is_default_relay(&addr));
    }

    #[test]
    fn the_resolved_default_rendezvous_is_recognised() {
        let Some(host) = crate::defaults::DEFAULT_RENDEZVOUS else {
            return;
        };
        let addr = format!("/dns4/{host}/udp/36521/quic-v1/p2p/12D3KooWanything");
        assert!(is_default_rendezvous(&addr));
    }

    /// A server on the default host but a different port is somebody's own
    /// deployment. Treating it as the default would send joiners to the public
    /// one instead, so the port has to be part of the judgement.
    #[test]
    fn a_different_port_on_the_default_host_is_not_the_default() {
        let Some(host) = crate::defaults::DEFAULT_RELAY else {
            return;
        };
        let addr = format!("/dns4/{host}/tcp/9999/p2p/12D3KooWanything");
        assert!(!is_default_relay(&addr));
    }

    #[test]
    fn another_host_is_not_the_default() {
        assert!(!is_default_relay(
            "/dns4/relay.example.com/tcp/36522/p2p/12D3KooWanything"
        ));
        assert!(!is_default_rendezvous(
            "/ip4/203.0.113.17/udp/36521/quic-v1/p2p/12D3KooWanything"
        ));
    }

    /// The relay runs TCP and the rendezvous server QUIC, on different ports.
    /// Confusing the two would put a circuit address in the discovery list.
    #[test]
    fn the_two_services_are_not_interchangeable() {
        let Some(host) = crate::defaults::DEFAULT_RELAY else {
            return;
        };
        let relay = format!("/dns4/{host}/tcp/36522/p2p/12D3KooWanything");
        let rendezvous = format!("/dns4/{host}/udp/36521/quic-v1/p2p/12D3KooWanything");
        assert!(!is_default_rendezvous(&relay));
        assert!(!is_default_relay(&rendezvous));
    }
}
