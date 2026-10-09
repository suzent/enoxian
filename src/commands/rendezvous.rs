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

/// Whether `addr` is the compiled-in default rendezvous server, judged on host,
/// transport and port — not the peer ID, which the joiner fetches fresh anyway.
/// A wrong `false` only costs bytes: the address is embedded verbatim.
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

/// The `host:port` to reach a rendezvous server's HTTP endpoint, read back out
/// of the multiaddr `resolve` produced for it.
///
/// The UDP port in a rendezvous address is the same port the server answers
/// HTTP on — see `resolve`, which builds the address from it — so the port has
/// to come along. Dropping it silently sends every request to the default port,
/// which is right only by coincidence.
///
/// `None` for an address with no host component.
pub fn http_endpoint_of(addr: &str) -> Option<String> {
    let parts: Vec<&str> = addr.split('/').filter(|p| !p.is_empty()).collect();
    let host_at = parts
        .iter()
        .position(|p| matches!(*p, "dns4" | "dns6" | "dnsaddr" | "ip4" | "ip6"))?;
    let host = parts.get(host_at + 1)?;

    let port = parts
        .iter()
        .position(|p| matches!(*p, "udp" | "tcp"))
        .and_then(|i| parts.get(i + 1))
        .and_then(|p| p.parse::<u16>().ok());

    Some(match port {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

/// Whether two `host[:port]` strings name the same server — host and port both.
///
/// Normalising rather than comparing strings, because one side usually carries
/// an explicit port and the other leaves the default implied: `relay:36521` and
/// `relay` are the same endpoint, `relay:45561` is not.
pub fn same_endpoint(a: &str, b: &str) -> bool {
    split_host_port(a, 36521) == split_host_port(b, 36521)
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

    /// The port must survive. A rendezvous server on a non-default port is the
    /// normal case when self-hosting or testing, and dropping it sends every
    /// HTTP request to the default port instead — which fails as a 404 far from
    /// where the mistake was made.
    #[test]
    fn an_http_endpoint_keeps_its_port() {
        assert_eq!(
            http_endpoint_of("/dns4/relay.enoxian.com/udp/36521/quic-v1/p2p/12D3KooWx").as_deref(),
            Some("relay.enoxian.com:36521")
        );
        assert_eq!(
            http_endpoint_of("/dns4/localhost/udp/45561/quic-v1/p2p/12D3KooWx").as_deref(),
            Some("localhost:45561")
        );
        assert_eq!(
            http_endpoint_of("/ip4/203.0.113.17/tcp/36522/p2p/12D3KooWx").as_deref(),
            Some("203.0.113.17:36522")
        );
        assert_eq!(http_endpoint_of("/p2p/12D3KooWx"), None);
        assert_eq!(http_endpoint_of(""), None);
    }

    /// The default is recognised whether or not a port came with it, so a stock
    /// circle's short invite does not carry a redundant hostname.
    #[test]
    fn the_default_endpoint_is_matched_through_an_implied_port() {
        assert!(same_endpoint(
            "relay.enoxian.com:36521",
            "relay.enoxian.com"
        ));
        assert!(same_endpoint("relay.enoxian.com", "relay.enoxian.com"));
        assert!(!same_endpoint(
            "other.example.com:36521",
            "relay.enoxian.com"
        ));
    }

    /// A different port on the default host is somebody's own server. Folding
    /// it into the default would upload the blob to 36521 rather than the
    /// server `--rendezvous` picked, and leave the link unable to name it.
    #[test]
    fn a_different_port_on_the_default_host_is_a_different_endpoint() {
        assert!(!same_endpoint(
            "relay.enoxian.com:45561",
            "relay.enoxian.com"
        ));
        assert!(!same_endpoint(
            "relay.enoxian.com:45561",
            "relay.enoxian.com:36521"
        ));
    }

    #[test]
    fn the_resolved_default_rendezvous_is_recognised() {
        let Some(host) = crate::defaults::DEFAULT_RENDEZVOUS else {
            return;
        };
        let addr = format!("/dns4/{host}/udp/36521/quic-v1/p2p/12D3KooWanything");
        assert!(is_default_rendezvous(&addr));
    }

    #[test]
    fn another_host_is_not_the_default() {
        assert!(!is_default_rendezvous(
            "/ip4/203.0.113.17/udp/36521/quic-v1/p2p/12D3KooWanything"
        ));
    }
}
