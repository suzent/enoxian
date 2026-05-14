use anyhow::{Context, Result};
use std::net::SocketAddr;
use tracing::{info, warn};

use crate::{api, cli::ServeArgs, config, daemon::DaemonState, lifecycle};

pub async fn run(args: ServeArgs) -> Result<()> {
    let all_configs = config::load_all().context("failed to load circle configs")?;
    if all_configs.is_empty() {
        anyhow::bail!("no circles found — run `enoch init` to create one");
    }

    let active: Vec<_> = all_configs.iter().filter(|c| !c.disabled).collect();
    info!(
        "Starting enochd — {} circle(s) found ({} active)",
        all_configs.len(),
        active.len()
    );

    let daemon = DaemonState::new();

    for config in active {
        lifecycle::spawn_circle(config.clone(), daemon.clone()).await?;
    }

    // Hot-reload: periodically check for new circles added while daemon is running.
    {
        let d = daemon.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
            loop {
                interval.tick().await;
                let Ok(cfgs) = config::load_all() else { continue };
                for cfg in cfgs {
                    if !cfg.disabled && !d.is_active(&cfg.circle_id) {
                        info!("[hot-reload] starting circle '{}'", cfg.circle_name);
                        if let Err(e) = lifecycle::spawn_circle(cfg, d.clone()).await {
                            warn!("[hot-reload] failed: {e}");
                        }
                    }
                }
            }
        });
    }

    let app = api::router(daemon);
    let http_addr = SocketAddr::from(([0, 0, 0, 0], args.port));
    let listener = tokio::net::TcpListener::bind(http_addr)
        .await
        .with_context(|| format!("failed to bind HTTP server on :{}", args.port))?;
    info!("HTTP/WS listening on :{}", args.port);

    axum::serve(listener, app).await.context("axum server error")?;
    Ok(())
}
