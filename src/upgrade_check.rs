//! Asking a Circle's bootstrap server whether this client is still served.
//!
//! A bootstrap server publishes `min_client_version` on `/version`. When a
//! change lands that old and new clients cannot speak across, the server raises
//! it, and a client below it says so through the status API instead of simply
//! never connecting again.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// How often a running Circle asks again.
pub const INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// A server's answer that this client is too old for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpgradeNotice {
    /// The server's `min_client_version`.
    pub min_version: String,
    /// The `host[:port]` that said so.
    pub server: String,
}

#[derive(Deserialize)]
struct VersionResponse {
    #[serde(default)]
    min_client_version: Option<String>,
}

/// The bootstrap hosts a Circle uses: its configured rendezvous servers, or the
/// compiled-in default when it has none.
pub fn hosts(rendezvous_addrs: &[String]) -> Vec<String> {
    let mut hosts: Vec<String> = rendezvous_addrs
        .iter()
        .filter_map(|addr| crate::commands::rendezvous::http_endpoint_of(addr))
        .collect();
    if rendezvous_addrs.is_empty() {
        hosts.extend(crate::defaults::DEFAULT_RENDEZVOUS.map(str::to_string));
    }
    hosts.dedup();
    hosts
}

/// Ask each host in turn; the first that says this client is too old wins.
///
/// An unreachable host, or one that predates `min_client_version`, counts as no
/// objection — a network blip must not tell anyone to upgrade.
pub async fn check(hosts: &[String]) -> Option<UpgradeNotice> {
    let client = crate::outbound::client();
    for host in hosts {
        let url = format!("{}/version", crate::outbound::http_base(host));
        let Ok(resp) = client.get(&url).send().await else {
            continue;
        };
        let Ok(body) = resp.json::<VersionResponse>().await else {
            continue;
        };
        if let Some(notice) = notice_from(host, body.min_client_version.as_deref()) {
            return Some(notice);
        }
    }
    None
}

fn notice_from(host: &str, min_version: Option<&str>) -> Option<UpgradeNotice> {
    let min_version = min_version?;
    crate::version::older_than(crate::version::VERSION, min_version)?.then(|| UpgradeNotice {
        min_version: min_version.to_string(),
        server: host.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minimum_above_this_build_is_a_notice() {
        let notice = notice_from("relay.example", Some("999.0.0")).unwrap();
        assert_eq!(notice.min_version, "999.0.0");
        assert_eq!(notice.server, "relay.example");
    }

    #[test]
    fn no_minimum_or_an_older_one_or_garbage_is_no_notice() {
        assert_eq!(notice_from("relay.example", None), None);
        assert_eq!(notice_from("relay.example", Some("0.0.1")), None);
        assert_eq!(
            notice_from("relay.example", Some(crate::version::VERSION)),
            None
        );
        assert_eq!(notice_from("relay.example", Some("soon")), None);
    }

    /// End to end over HTTP: an unreachable host is skipped, and a server
    /// asking for a newer version than this build produces the notice.
    #[tokio::test]
    async fn check_reads_min_client_version_from_a_live_server() {
        let app = axum::Router::new().route(
            "/version",
            axum::routing::get(|| async {
                axum::Json(serde_json::json!({
                    "version": "999.0.0",
                    "min_client_version": "999.0.0",
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let live = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let dead = "127.0.0.1:1".to_string();
        let notice = check(&[dead, live.clone()]).await.unwrap();
        assert_eq!(notice.min_version, "999.0.0");
        assert_eq!(notice.server, live);
    }

    #[test]
    fn hosts_come_from_configured_rendezvous_or_the_default() {
        let configured = hosts(&[
            "/dns4/relay.example/udp/45561/quic-v1/p2p/12D3KooWx".to_string(),
            "/dns4/relay.example/udp/45561/quic-v1/p2p/12D3KooWx".to_string(),
        ]);
        assert_eq!(configured, vec!["relay.example:45561".to_string()]);

        let default = hosts(&[]);
        assert_eq!(
            default,
            crate::defaults::DEFAULT_RENDEZVOUS
                .map(str::to_string)
                .into_iter()
                .collect::<Vec<_>>()
        );
    }
}
