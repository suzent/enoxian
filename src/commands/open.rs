use std::time::Duration;

use anyhow::Result;

pub async fn run(daemon_root: &str) -> Result<()> {
    let root = daemon_root.trim_end_matches('/');
    ensure_daemon_running(root).await;
    let url = format!("{root}/app");
    println!("Opening {url}");
    open::that(&url).map_err(|e| anyhow::anyhow!("failed to open browser: {e}"))?;
    Ok(())
}

async fn ensure_daemon_running(root: &str) {
    let probe = reqwest::Client::builder()
        .timeout(Duration::from_millis(400))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let app_url = format!("{root}/app");
    if probe.get(&app_url).send().await.is_ok() {
        return;
    }

    let Some(port) = local_port(root) else {
        return;
    };
    let port_override = (port != 36521).then_some(port);
    if let Err(e) = crate::commands::start::run(port_override).await {
        eprintln!("warning: could not auto-start daemon: {e}");
        return;
    }

    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if probe.get(&app_url).send().await.is_ok() {
            break;
        }
    }
}

fn local_port(root: &str) -> Option<u16> {
    let rest = root
        .strip_prefix("http://127.0.0.1")
        .or_else(|| root.strip_prefix("http://localhost"))?;
    if rest.is_empty() {
        return Some(36521);
    }
    let port_str = rest.strip_prefix(':')?.split('/').next()?;
    port_str.parse::<u16>().ok()
}

#[cfg(test)]
mod tests {
    use super::local_port;

    #[test]
    fn parses_local_daemon_port() {
        assert_eq!(local_port("http://127.0.0.1:36521"), Some(36521));
        assert_eq!(local_port("http://localhost:4000"), Some(4000));
        assert_eq!(local_port("http://127.0.0.1"), Some(36521));
        assert_eq!(local_port("https://example.com:36521"), None);
    }
}
