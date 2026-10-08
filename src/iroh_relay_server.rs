//! An Iroh relay inside `enox bootstrap serve`, behind `iroh-relay-server`.
//!
//! Members of an Iroh Circle meet through a relay before they can punch a
//! direct path, and fall back to it when they cannot. Running it here keeps a
//! self-hosted Circle at one server and one binary, as with libp2p.
//!
//! In production it serves HTTPS on 443 with a Let's Encrypt certificate
//! (TLS-ALPN-01, so nothing else may hold 443), plain HTTP on 80 for Iroh's
//! captive-portal probe, and QUIC address discovery on 7842. Dev mode serves
//! plain HTTP on one port and is for local testing only.

use std::net::{Ipv4Addr, SocketAddr};

use anyhow::{Context, Result};
use iroh_relay::server::{
    AcmeConfig, CertConfig, QuicConfig, RelayConfig, Server, ServerConfig, TlsConfig,
};
use tracing::info;

/// How to run the relay, from `enox bootstrap serve` flags.
#[derive(Debug, Clone)]
pub enum RelayMode {
    /// Plain HTTP on this port, no TLS. Local testing only.
    Dev { port: u16 },
    /// HTTPS with a Let's Encrypt certificate for `domain`.
    Production {
        domain: String,
        https_port: u16,
        http_port: u16,
        quic_port: Option<u16>,
        contact: Option<String>,
        /// Let's Encrypt's staging directory: untrusted certificates, but no
        /// production rate limits while a deployment is being tried out.
        staging: bool,
    },
}

/// A running relay and the URL clients reach it at. Dropping it stops it.
pub struct Relay {
    pub server: Server,
    pub url: String,
}

pub async fn start(mode: RelayMode) -> Result<Relay> {
    // The server builds rustls configs from the process-wide provider.
    let _ = rustls::crypto::ring::default_provider().install_default();
    match mode {
        RelayMode::Dev { port } => {
            let mut config = ServerConfig::default();
            config.relay = Some(RelayConfig::new((Ipv4Addr::UNSPECIFIED, port)));
            let server = Server::spawn(config)
                .await
                .context("starting the Iroh relay")?;
            let addr = server.http_addr().context("relay has no HTTP address")?;
            let url = format!("http://{}:{}", Ipv4Addr::LOCALHOST, addr.port());
            info!("  Iroh relay (dev, plain HTTP): {url}");
            Ok(Relay { server, url })
        }
        RelayMode::Production {
            domain,
            https_port,
            http_port,
            quic_port,
            contact,
            staging,
        } => {
            let cache = crate::config::enoxian_dir()?.join("acme");
            std::fs::create_dir_all(&cache).context("creating the ACME cache directory")?;
            let mut acme = AcmeConfig::letsencrypt(!staging)
                .domains(vec![domain.clone()])
                .cache_path(cache);
            if let Some(contact) = contact {
                acme = acme.contact(vec![format!("mailto:{contact}")]);
            }
            let tls_builder = rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .context("TLS configuration")?
            .with_no_client_auth();

            let mut relay = RelayConfig::new((Ipv4Addr::UNSPECIFIED, http_port));
            relay.tls = Some(TlsConfig::new(
                SocketAddr::from((Ipv4Addr::UNSPECIFIED, https_port)),
                CertConfig::LetsEncrypt {
                    acme_config: acme,
                    server_config_builder: tls_builder,
                },
            ));
            let mut config = ServerConfig::default();
            config.relay = Some(relay);
            config.quic = quic_port.map(|port| QuicConfig::new((Ipv4Addr::UNSPECIFIED, port)));
            let server = Server::spawn(config)
                .await
                .context("starting the Iroh relay")?;
            let url = if https_port == 443 {
                format!("https://{domain}")
            } else {
                format!("https://{domain}:{https_port}")
            };
            info!(
                "  Iroh relay: {url} (Let's Encrypt{})",
                if staging { " staging" } else { "" }
            );
            Ok(Relay { server, url })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::{
        endpoint::{presets, PortmapperConfig},
        Endpoint, EndpointAddr, RelayMode as ClientRelayMode, RelayUrl, SecretKey,
    };

    /// Two endpoints that can only meet through the embedded relay do meet.
    #[tokio::test]
    async fn iroh_endpoints_connect_through_the_embedded_relay() {
        const ALPN: &[u8] = b"enoxian-relay-test";
        let relay = start(RelayMode::Dev { port: 0 }).await.unwrap();
        let url: RelayUrl = relay.url.parse().unwrap();
        let endpoint = || async {
            Endpoint::builder(presets::Minimal)
                .secret_key(SecretKey::generate())
                .alpns(vec![ALPN.to_vec()])
                .relay_mode(ClientRelayMode::custom([url.clone()]))
                .portmapper_config(PortmapperConfig::Disabled)
                .clear_ip_transports()
                .bind()
                .await
                .unwrap()
        };
        let (a, b) = (endpoint().await, endpoint().await);
        tokio::time::timeout(std::time::Duration::from_secs(10), a.online())
            .await
            .expect("endpoint never registered with the relay");

        let server = a.clone();
        tokio::spawn(async move {
            let conn = server.accept().await.unwrap().await.unwrap();
            let (mut send, mut recv) = conn.accept_bi().await.unwrap();
            let got = recv.read_to_end(64).await.unwrap();
            send.write_all(&got).await.unwrap();
            send.finish().unwrap();
            conn.closed().await;
        });

        let conn = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            b.connect(EndpointAddr::new(a.id()).with_relay_url(url.clone()), ALPN),
        )
        .await
        .expect("connecting through the relay timed out")
        .unwrap();
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        send.write_all(b"through the relay").await.unwrap();
        send.finish().unwrap();
        assert_eq!(recv.read_to_end(64).await.unwrap(), b"through the relay");
        assert!(conn.paths().iter().all(|path| path.is_relay()));
        conn.close(0u32.into(), b"done");
        b.close().await;
        a.close().await;
    }
}
