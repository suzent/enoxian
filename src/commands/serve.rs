use anyhow::{Context, Result};
use std::net::SocketAddr;
use tracing::{info, warn};

#[cfg(not(debug_assertions))]
#[derive(rust_embed::Embed)]
#[folder = "static/"]
struct FrontendAssets;

use crate::{
    api, cli::ServeArgs, config, daemon::DaemonState, identity::DeviceIdentity, lifecycle,
};

pub async fn run(args: ServeArgs) -> Result<()> {
    // ── Device identity: first-run setup ─────────────────────────────────────
    // On first launch, prompt for a device label. Subsequent starts auto-load
    // the saved identity silently. The identity is used to derive stable
    // per-circle keypairs (see docs/concepts/security.md).
    let device = ensure_identity()?;
    info!(
        "Device identity: {} ({})",
        device.device_label,
        device.user_handle.as_deref().unwrap_or("no user linked")
    );

    let all_configs = config::load_all().unwrap_or_default();

    let active: Vec<_> = all_configs.iter().filter(|c| !c.disabled).collect();
    info!(
        "Starting Enoxian — {} circle(s) found ({} active)",
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
                let Ok(cfgs) = config::load_all() else {
                    continue;
                };
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

    // Local API auth token — generated on first start, presented by the CLI and
    // the frontend. The API is a privileged control plane; the token stops a
    // local process (e.g. a malicious webpage) from driving it. See api::auth.
    let token = api::auth::load_or_create().context("initializing API token")?;
    info!("API token at {}", api::auth::token_path()?.display());

    // The API router is token-guarded. The frontend static assets must NOT be
    // (the browser loads them before it has the token), so they are built as a
    // separate un-authed router and merged. The token defense for the browser is
    // that the token is injected into the served HTML, which a cross-origin page
    // cannot read — not that the HTML itself is auth-gated.
    let api_app = api::router(daemon.clone(), Some(token.clone()));

    let mut app = api_app;

    // Release binaries embed the production frontend so the one-file installer
    // can always serve /app. Debug builds retain the on-disk lookup used by the
    // Vite/local development workflow.
    #[cfg(not(debug_assertions))]
    {
        use axum::{
            body::Body,
            http::{header, StatusCode, Uri},
            response::{Html, IntoResponse, Response},
            routing::get,
        };

        let index_html = FrontendAssets::get("index.html")
            .context("release binary is missing embedded frontend/index.html")?;
        let index_html = String::from_utf8(index_html.data.into_owned())
            .context("embedded frontend/index.html is not UTF-8")?;
        let injected = inject_frontend_token(&index_html, &token);
        let index_route = || {
            get({
                let html = injected.clone();
                move || async move { Html(html) }
            })
        };

        async fn embedded_asset(uri: Uri) -> Response {
            let path = uri.path().trim_start_matches('/');
            let Some(asset) = FrontendAssets::get(path) else {
                return StatusCode::NOT_FOUND.into_response();
            };
            Response::builder()
                .header(header::CONTENT_TYPE, frontend_content_type(path))
                .body(Body::from(asset.data.into_owned()))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }

        app = app
            .route("/app", index_route())
            .route("/app/", index_route())
            .fallback(embedded_asset);
        info!("Serving embedded frontend at /app");
    }

    #[cfg(debug_assertions)]
    {
        // Serve the compiled frontend at /app when it exists on disk. Try the
        // crate-root static dir first, then the parent workspace layout.
        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let static_dir = [
            manifest.join("static"),
            manifest
                .parent()
                .map(|p| p.join("static"))
                .unwrap_or_default(),
        ]
        .into_iter()
        .find(|p| p.join("index.html").exists())
        .unwrap_or_else(|| manifest.join("static"));
        if static_dir.join("index.html").exists() {
            use tower_http::services::ServeDir;
            let index_html =
                std::fs::read_to_string(static_dir.join("index.html")).unwrap_or_default();
            let injected = inject_frontend_token(&index_html, &token);
            let index_route = || {
                axum::routing::get({
                    let html = injected.clone();
                    move || async move { axum::response::Html(html) }
                })
            };
            // Built assets use absolute root paths (/assets/…, /logo.svg).
            app = app
                .route("/app", index_route())
                .route("/app/", index_route())
                .fallback_service(ServeDir::new(&static_dir));
            info!("Serving frontend at /app from {}", static_dir.display());
        }
    }

    // Bind to loopback by default — the API is a privileged control plane, not
    // a public endpoint. `--bind <ip>` or `--bind-lan` opt into wider exposure.
    let ip = match args.bind {
        Some(ip) => ip,
        None if args.bind_lan => std::net::IpAddr::from([0, 0, 0, 0]),
        None => std::net::IpAddr::from([127, 0, 0, 1]),
    };
    if !ip.is_loopback() {
        warn!(
            "API bound to {ip} (non-loopback) — this control plane is now reachable off-host. \
             Ensure the network is trusted; the API token is required but exposure widens risk."
        );
    }
    let http_addr = SocketAddr::new(ip, args.port);
    let listener = tokio::net::TcpListener::bind(http_addr)
        .await
        .with_context(|| format!("failed to bind HTTP server on {http_addr}"))?;
    info!("HTTP/WS listening on {http_addr}");

    let shutdown = daemon.shutdown_token.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await
        .context("axum server error")?;

    info!("Enoxian stopped");
    Ok(())
}

fn inject_frontend_token(index_html: &str, token: &str) -> String {
    index_html.replacen(
        "</head>",
        &format!("<script>window.__ENOX_TOKEN__=\"{token}\";</script></head>"),
        1,
    )
}

#[cfg(not(debug_assertions))]
fn frontend_content_type(path: &str) -> &'static str {
    match std::path::Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
    {
        "css" => "text/css; charset=utf-8",
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// Load the device identity or run a first-time setup prompt.
/// When running non-interactively (no TTY / ENOXIAN_DEVICE_LABEL set),
/// auto-generates with the hostname as the label.
fn ensure_identity() -> Result<DeviceIdentity> {
    if DeviceIdentity::exists() {
        return DeviceIdentity::load();
    }

    // Check env-var override first (for automated / headless setups).
    let label_from_env = std::env::var("ENOXIAN_DEVICE_LABEL")
        .ok()
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
        if trimmed.is_empty() {
            default
        } else {
            trimmed
        }
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
        extern "C" {
            fn isatty(fd: std::os::raw::c_int) -> std::os::raw::c_int;
        }
        unsafe { isatty(std::io::stdin().as_raw_fd()) != 0 }
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn hostname_label() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| {
            s.trim()
                .split('.')
                .next()
                .unwrap_or("device")
                .to_lowercase()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "device".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontend_token_is_injected_before_head_closes() {
        let html = "<html><head><title>Enoxian</title></head><body></body></html>";
        let injected = inject_frontend_token(html, "abc123");
        assert!(injected.contains("<script>window.__ENOX_TOKEN__=\"abc123\";</script></head>"));
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn release_frontend_contains_entry_point_and_assets() {
        assert!(FrontendAssets::get("index.html").is_some());
        assert!(FrontendAssets::get("logo.svg").is_some());
        assert_eq!(
            frontend_content_type("assets/app.js"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            frontend_content_type("assets/app.css"),
            "text/css; charset=utf-8"
        );
    }
}
