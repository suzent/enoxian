use anyhow::{Context, Result};
use std::net::SocketAddr;
use tracing::{info, warn};

use crate::{api, cli::ServeArgs, config, daemon::DaemonState, identity::DeviceIdentity, lifecycle};

pub async fn run(args: ServeArgs) -> Result<()> {
    // ── Device identity: first-run setup ─────────────────────────────────────
    // On first launch, prompt for a device label. Subsequent starts auto-load
    // the saved identity silently. The identity is used to derive stable
    // per-circle keypairs (see docs/plan/identity.md).
    let device = ensure_identity()?;
    info!(
        "Device identity: {} ({})",
        device.device_label,
        device.user_handle.as_deref().unwrap_or("no user linked")
    );

    let all_configs = config::load_all().unwrap_or_default();

    let active: Vec<_> = all_configs.iter().filter(|c| !c.disabled).collect();
    info!(
        "Starting enoxd — {} circle(s) found ({} active)",
        all_configs.len(),
        active.len()
    );
    if all_configs.is_empty() {
        info!("No circles yet — waiting for `enox init` or POST /api/init via the frontend.");
    }

    let daemon = DaemonState::new();

    for config in active {
        if let Err(e) = lifecycle::spawn_circle(config.clone(), daemon.clone()).await {
            warn!("Failed to start circle '{}': {e}", config.circle_name);
        }
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

    let mut app = api::router(daemon.clone());

    // Serve the compiled frontend at /app (built by `npm run build` in frontend/)
    let static_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap_or(&std::path::PathBuf::from("."))
        .join("static");
    if static_dir.exists() {
        use tower_http::services::{ServeDir, ServeFile};
        let serve = ServeDir::new(&static_dir)
            .fallback(ServeFile::new(static_dir.join("index.html")));
        app = app.nest_service("/app", serve);
        info!("Serving frontend at /app from {}", static_dir.display());
    }

    let http_addr = SocketAddr::from(([0, 0, 0, 0], args.port));
    let listener = tokio::net::TcpListener::bind(http_addr)
        .await
        .with_context(|| format!("failed to bind HTTP server on :{}", args.port))?;
    info!("HTTP/WS listening on :{}", args.port);

    let shutdown = daemon.shutdown_token.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await
        .context("axum server error")?;

    info!("enoxd stopped");
    Ok(())
}

/// Load the device identity or run a first-time setup prompt.
/// When running non-interactively (no TTY / ENOXIAN_DEVICE_LABEL set),
/// auto-generates with the hostname as the label.
fn ensure_identity() -> Result<DeviceIdentity> {
    if DeviceIdentity::exists() {
        return DeviceIdentity::load();
    }

    // Check env-var override first (for automated / headless setups).
    let label_from_env = std::env::var("ENOXIAN_DEVICE_LABEL").ok()
        .filter(|s| !s.is_empty());

    let label = if let Some(l) = label_from_env {
        eprintln!("enoxian: first run — creating device identity '{l}'");
        l
    } else if !is_interactive() {
        // Non-interactive: auto-generate from hostname.
        let label = hostname_label();
        eprintln!("enoxian: first run — creating device identity '{label}' (set ENOXIAN_DEVICE_LABEL to override)");
        label
    } else {
        // Interactive TTY: prompt the user.
        let default = hostname_label();
        eprint!("enoxian: first run!\nDevice label [{default}]: ");
        let mut input = String::new();
        let _ = std::io::stdin().read_line(&mut input);
        let trimmed = input.trim().to_string();
        if trimmed.is_empty() { default } else { trimmed }
    };

    let device = DeviceIdentity::generate(label);
    device.save()?;
    eprintln!(
        "enoxian: identity saved to ~/.enoxian/identity.toml\n\
         Run `enox identity` to view or link a user account."
    );
    Ok(device)
}

fn is_interactive() -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        extern "C" { fn isatty(fd: std::os::raw::c_int) -> std::os::raw::c_int; }
        unsafe { isatty(std::io::stdin().as_raw_fd()) != 0 }
    }
    #[cfg(not(unix))]
    { true }
}

fn hostname_label() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().split('.').next().unwrap_or("device").to_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "device".to_string())
}
