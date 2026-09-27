use anyhow::Result;
use std::io::{self, Write as _};

use crate::{config, resolve};

pub async fn run(
    client: &reqwest::Client,
    daemon_base: &str,
    circle_hint: Option<&str>,
    yes: bool,
    admin_to: Option<&str>,
    force: bool,
) -> Result<()> {
    let configs = config::load_all()?;
    let cfg = match circle_hint {
        Some(h) => resolve::resolve(h, &configs)?,
        None => resolve::resolve_default(&configs)?,
    }
    .clone();

    if !yes {
        print!(
            "Leave '{}' ({})? This removes all local config. [y/N] ",
            cfg.circle_name, cfg.circle_id
        );
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    // The daemon leaves for us: an admin has to hand its key over while the
    // circle still runs, and only then may the directory go.
    let url = format!("{}/circles/{}/api/leave", daemon_base, cfg.circle_id);
    let body = serde_json::json!({ "admin_to": admin_to, "force": force });
    match client.post(&url).json(&body).send().await {
        Ok(response) if response.status().is_success() => {
            let reply: serde_json::Value = response.json().await.unwrap_or_default();
            if let Some(successor) = reply["admin_handed_to"].as_str() {
                println!("✦ Handed admin over to {successor}.");
            }
        }
        Ok(response) => {
            let status = response.status();
            let reply: serde_json::Value = response.json().await.unwrap_or_default();
            let error = reply["error"].as_str().unwrap_or("unknown error");
            anyhow::bail!("did not leave '{}' ({status}): {error}", cfg.circle_name);
        }
        Err(_) => {
            // No daemon: nothing is running to hand the admin key to anyone.
            let dir = config::circle_dir(&cfg.circle_id)?;
            if dir.join("admin.key").exists() && !force {
                anyhow::bail!(
                    "this device is the admin of '{}', and Enoxian is not running, so \
                     the admin key cannot be handed over — run `enox start` and try \
                     again, or pass --force to give admin up for good",
                    cfg.circle_name
                );
            }
            if dir.exists() {
                std::fs::remove_dir_all(&dir)
                    .map_err(|e| anyhow::anyhow!("failed to remove {}: {e}", dir.display()))?;
            }
        }
    }

    println!("✦ Left circle '{}'. Config removed.", cfg.circle_name);
    println!(
        "  Note: your workspace files at {} are untouched.",
        cfg.workspace_dir
    );
    Ok(())
}
