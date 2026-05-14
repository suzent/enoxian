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
    updated.disabled = true;
    config::save(&updated)?;

    // Best-effort stop — daemon may not be running
    let url = format!("{}/circles/{}/stop", daemon_base, cfg.circle_id);
    let _ = client.post(&url).send().await;

    println!("✦ Circle '{}' disabled — enochd will skip it on next start.", cfg.circle_name);
    println!("  Re-enable with: enoch enable");
    Ok(())
}
