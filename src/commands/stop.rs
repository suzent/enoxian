use anyhow::Result;
use std::time::Duration;

const STOP_REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

pub async fn run(client: &reqwest::Client, root: &str) -> Result<()> {
    run_with_timeout(client, root, STOP_REQUEST_TIMEOUT).await
}

async fn run_with_timeout(
    client: &reqwest::Client,
    root: &str,
    request_timeout: Duration,
) -> Result<()> {
    let resp = tokio::time::timeout(
        request_timeout,
        client.post(format!("{root}/shutdown")).send(),
    )
    .await;

    match resp {
        Ok(Ok(r)) if r.status().is_success() => println!("✓ Enoxian stopped"),
        Ok(Ok(r)) => anyhow::bail!("unexpected response: {}", r.status()),
        Ok(Err(_)) => anyhow::bail!("Enoxian is not running (or not reachable at {root})"),
        Err(_) => anyhow::bail!(
            "timed out waiting for Enoxian at {root}; use `enox service stop` for a managed service"
        ),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stop_request_times_out_when_daemon_accepts_but_never_responds() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });

        let error = run_with_timeout(
            &reqwest::Client::new(),
            &format!("http://{address}"),
            Duration::from_millis(25),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("timed out waiting for Enoxian"));
        server.abort();
    }
}
