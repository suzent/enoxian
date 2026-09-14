//! HTTP client for hosts that are not this machine's daemon.
//!
//! The CLI's usual client carries `Authorization: Bearer <api.token>` as a
//! default header, because nearly every request it makes goes to the local
//! daemon, which requires it. That token is a privileged local credential: it
//! authorises the whole management API.
//!
//! A few commands talk to somebody else's server over plain HTTP — the pairing
//! mailbox during `enox link`, and `/peer-id` on a bootstrap host while
//! resolving a rendezvous or relay address. Reusing the daemon's client for
//! those hands the token to whoever is running that host, before the user has
//! confirmed anything. Use this client for them instead: same settings, no
//! credentials.

use std::time::Duration;

/// A client with no ambient credentials, for requests leaving this machine.
pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

#[cfg(test)]
mod tests {
    /// The point of this module. If a default `Authorization` header ever gets
    /// added here, every pairing request starts leaking the daemon token to a
    /// third-party server.
    #[tokio::test]
    async fn the_outbound_client_sends_no_authorization_header() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let captured = seen.clone();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0u8; 4096];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                captured
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf[..n]).to_string());
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                    .await;
            }
        });

        let _ = super::client()
            .get(format!("http://{addr}/pair/abc/offer"))
            .send()
            .await;

        let request = seen.lock().unwrap().join("");
        assert!(
            !request.to_ascii_lowercase().contains("authorization"),
            "outbound request carried credentials:\n{request}"
        );
    }
}
