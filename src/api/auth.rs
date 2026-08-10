//! Local API authentication.
//!
//! The `enoxd` HTTP API is a **privileged control plane**: it can add agents,
//! arm push-mode (letting a chat mention run a process), start/stop circles, and
//! more. Because it is served over loopback, the threat is not a remote attacker
//! but a **local one** — most importantly a malicious webpage in the operator's
//! browser doing `fetch("http://127.0.0.1:36521/...")`. Loopback binding plus a
//! CORS allowlist reduce that surface; this token closes it.
//!
//! The token is a random secret stored at `~/.enoxian/api.token`, readable only
//! by processes with filesystem access to the operator's home dir — which a
//! webpage does not have. Local clients present it as `Authorization: Bearer
//! <token>`:
//!
//! - the `enox` CLI reads the file and sends the header;
//! - the frontend receives the token injected into its served HTML (a
//!   cross-origin page cannot read that response, so it cannot steal the token).
//!
//! Generated once on first daemon start.

use anyhow::{Context, Result};
use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::Response,
};
use rand::RngCore;
use std::path::PathBuf;
use std::sync::Arc;

/// Path to the API token file.
pub fn token_path() -> Result<PathBuf> {
    Ok(crate::config::enoxian_dir()?.join("api.token"))
}

/// Load the token, generating it on first use. Written with owner-only
/// permissions where the platform supports it.
pub fn load_or_create() -> Result<String> {
    let path = token_path()?;
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let t = existing.trim().to_string();
        if !t.is_empty() {
            return Ok(t);
        }
    }
    let token = generate();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, &token).with_context(|| format!("writing {}", path.display()))?;
    restrict_permissions(&path);
    Ok(token)
}

/// Read the token if it exists (for clients). Returns None if absent.
pub fn load() -> Option<String> {
    let path = token_path().ok()?;
    let t = std::fs::read_to_string(path).ok()?.trim().to_string();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

fn generate() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Middleware: require the shared token on every request, via
/// `Authorization: Bearer <token>` or a `?token=<token>` query parameter.
/// The query form exists because browsers cannot set headers on WebSocket
/// (`/ws/...`) or `EventSource` (SSE) connections.
pub async fn require_token(
    State(expected): State<Arc<String>>,
    req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    if request_token(&req).as_deref() == Some(expected.as_str()) {
        Ok(next.run(req).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

/// Extract the presented token from the Authorization header or query string.
fn request_token(req: &Request<Body>) -> Option<String> {
    // Authorization: Bearer <token>
    if let Some(auth) = req.headers().get(axum::http::header::AUTHORIZATION) {
        if let Ok(s) = auth.to_str() {
            if let Some(t) = s.strip_prefix("Bearer ") {
                return Some(t.trim().to_string());
            }
        }
    }
    // ?token=<token>  (WebSocket / SSE)
    req.uri().query().and_then(|q| {
        q.split('&').find_map(|kv| {
            kv.strip_prefix("token=").map(|v| {
                // minimal percent-decode is unnecessary for a hex token
                v.to_string()
            })
        })
    })
}

#[cfg(unix)]
fn restrict_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &std::path::Path) {
    // On Windows the file inherits the user profile ACL, which already restricts
    // it to the owner; no portable chmod equivalent is applied here.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_is_random_and_hex() {
        let a = generate();
        let b = generate();
        assert_ne!(a, b);
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
