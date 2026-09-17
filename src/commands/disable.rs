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

    // As in `enable`: the config write is durable either way, but say plainly
    // whether the running circle actually stopped.
    let url = format!("{}/circles/{}/stop", daemon_base, cfg.circle_id);
    println!(
        "✦ Circle '{}' disabled — Enoxian will skip it on next start.",
        cfg.circle_name
    );
    match client.post(&url).send().await {
        Err(_) => println!("  Enoxian is not running; nothing to stop."),
        Ok(response) if response.status().is_success() => println!("  Stopped."),
        Ok(response) if response.status() == reqwest::StatusCode::NOT_FOUND => {
            println!("  It was not running.")
        }
        Ok(response) => println!("  But the daemon did not stop it: {}", response.status()),
    }
    println!("  Re-enable with: enox enable");
    Ok(())
}
