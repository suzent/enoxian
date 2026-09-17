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

    // The config write above is the durable part. Whether the running daemon
    // picked the circle up is separate, and reporting success for both when
    // only the first happened sends people hunting in the wrong place.
    let url = format!("{}/circles/{}/start", daemon_base, cfg.circle_id);
    println!("✦ Circle '{}' enabled.", cfg.circle_name);
    match client.post(&url).send().await {
        Err(_) => {
            println!("  Enoxian is not running — it will start on next launch.");
        }
        Ok(response) if response.status().is_success() => {
            println!("  Started.");
        }
        Ok(response) if response.status() == reqwest::StatusCode::CONFLICT => {
            println!("  Already running.");
        }
        Ok(response) => {
            let status = response.status();
            let detail = response
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|body| body.get("error")?.as_str().map(str::to_owned))
                .unwrap_or_else(|| status.to_string());
            println!("  But the daemon did not start it: {detail}");
            println!("  It will be retried within ~10s (hot-reload); check `enox status`.");
        }
    }
    Ok(())
}
