use anyhow::Result;

use crate::{config, resolve};

pub async fn run(
    client: &reqwest::Client,
    daemon_base: &str,
    circle_hint: Option<&str>,
) -> Result<()> {
    let configs = config::load_all()?;
    let cfg = match circle_hint {
        Some(h) => resolve::resolve(h, &configs)?,
        None => resolve::resolve_default(&configs)?,
    }
    .clone();

    let mut updated = cfg.clone();
    updated.disabled = false;
    config::save(&updated)?;

    // Best-effort start — daemon may not be running (hot-reload will pick it up)
    let url = format!("{}/circles/{}/start", daemon_base, cfg.circle_id);
    let _ = client.post(&url).send().await;

    println!("✦ Circle '{}' enabled.", cfg.circle_name);
    println!("  If enochd is running it will start within ~10s (hot-reload).");
    Ok(())
}
