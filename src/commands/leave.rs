use anyhow::Result;
use std::io::{self, Write as _};

use crate::{config, resolve};

pub async fn run(
    client: &reqwest::Client,
    daemon_base: &str,
    circle_hint: Option<&str>,
    yes: bool,
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

    // Stop the circle in the daemon (best-effort)
    let url = format!("{}/circles/{}/stop", daemon_base, cfg.circle_id);
    let _ = client.post(&url).send().await;

    // Remove config directory
    let dir = config::circle_dir(&cfg.circle_id)?;
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|e| anyhow::anyhow!("failed to remove {}: {e}", dir.display()))?;
    }

    println!("✦ Left circle '{}'. Config removed.", cfg.circle_name);
    println!(
        "  Note: your workspace files at {} are untouched.",
        cfg.workspace_dir
    );
    Ok(())
}
