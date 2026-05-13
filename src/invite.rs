//! `enochian://` invite URI encoding, decoding, and expiry validation.
//!
//! Binary payload (48 bytes, base64url-no-pad):
//!   bytes  0-15  — circle UUID (big-endian, as returned by Uuid::as_bytes)
//!   bytes 16-47  — PSK (32 raw bytes)
//!
//! Full URI:
//!   enochian://v1/<b64payload>?expires=<RFC3339>&name=<str>&peer=<b64addr>
//!
//! `peer` is itself base64url-encoded so multiaddr slashes don't require
//! percent-encoding in the query string.

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

const SCHEME_PREFIX: &str = "enochian://v1/";

pub struct InvitePayload {
    pub circle_id:   String,
    pub psk_bytes:   [u8; 32],
    pub circle_name: Option<String>,
    pub expires_at:  DateTime<Utc>,
    pub peer_addr:   Option<String>,
}

// ── Encoding ──────────────────────────────────────────────────────────────────

pub fn encode(payload: &InvitePayload) -> String {
    let uuid = Uuid::parse_str(&payload.circle_id).expect("circle_id must be a valid UUID");
    let mut raw = [0u8; 48];
    raw[..16].copy_from_slice(uuid.as_bytes());
    raw[16..].copy_from_slice(&payload.psk_bytes);

    let b64 = URL_SAFE_NO_PAD.encode(raw);
    let expires = payload.expires_at.format("%Y-%m-%dT%H:%M:%SZ");
    let mut uri = format!("{SCHEME_PREFIX}{b64}?expires={expires}");

    if let Some(name) = &payload.circle_name {
        uri.push_str("&name=");
        uri.push_str(&percent_encode_component(name));
    }
    if let Some(peer) = &payload.peer_addr {
        // Base64url-encode the peer addr to avoid percent-encoding slashes
        uri.push_str("&peer=");
        uri.push_str(&URL_SAFE_NO_PAD.encode(peer.as_bytes()));
    }

    uri
}

// ── Decoding ──────────────────────────────────────────────────────────────────

pub fn decode(uri: &str) -> Result<InvitePayload> {
    let rest = uri
        .strip_prefix(SCHEME_PREFIX)
        .with_context(|| format!("not a valid enochian:// URI: {uri}"))?;

    let (b64, query) = rest.split_once('?').unwrap_or((rest, ""));

    let raw = URL_SAFE_NO_PAD
        .decode(b64)
        .context("invite URI payload is not valid base64url")?;

    if raw.len() != 48 {
        bail!("invite payload is {} bytes, expected 48", raw.len());
    }

    let uuid_bytes: [u8; 16] = raw[..16].try_into().unwrap();
    let circle_id = Uuid::from_bytes(uuid_bytes).to_string();

    let psk_bytes: [u8; 32] = raw[16..48].try_into().unwrap();

    let mut expires_at: Option<DateTime<Utc>> = None;
    let mut circle_name: Option<String> = None;
    let mut peer_addr: Option<String> = None;

    for pair in query.split('&').filter(|s| !s.is_empty()) {
        if let Some((key, val)) = pair.split_once('=') {
            match key {
                "expires" => {
                    expires_at = Some(
                        DateTime::parse_from_rfc3339(val)
                            .context("invalid expires timestamp in invite")?
                            .with_timezone(&Utc),
                    );
                }
                "name" => {
                    circle_name = Some(percent_decode_component(val));
                }
                "peer" => {
                    let bytes = URL_SAFE_NO_PAD
                        .decode(val)
                        .context("invalid base64url peer addr in invite")?;
                    peer_addr = Some(
                        String::from_utf8(bytes).context("peer addr is not valid UTF-8")?,
                    );
                }
                _ => {} // ignore unknown params for forward compatibility
            }
        }
    }

    let expires_at = expires_at.context("invite URI is missing required 'expires' parameter")?;

    Ok(InvitePayload { circle_id, psk_bytes, circle_name, expires_at, peer_addr })
}

// ── Expiry ────────────────────────────────────────────────────────────────────

pub fn check_expiry(payload: &InvitePayload) -> Result<()> {
    let now = Utc::now();
    if now > payload.expires_at {
        let ago = now - payload.expires_at;
        bail!(
            "invite expired {} ago (at {})",
            format_duration(ago),
            payload.expires_at.format("%Y-%m-%d %H:%M UTC")
        );
    }
    Ok(())
}

// ── TTL parsing ───────────────────────────────────────────────────────────────

/// Parse a human TTL string like "7d" or "24h" into a chrono Duration.
pub fn parse_ttl(s: &str) -> Result<Duration> {
    if let Some(days) = s.strip_suffix('d') {
        let n: i64 = days.parse().context("invalid number of days in TTL")?;
        Ok(Duration::days(n))
    } else if let Some(hours) = s.strip_suffix('h') {
        let n: i64 = hours.parse().context("invalid number of hours in TTL")?;
        Ok(Duration::hours(n))
    } else {
        bail!("invalid TTL '{}' — use e.g. '7d' or '24h'", s)
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn percent_encode_component(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
                vec![c]
            } else {
                // Encode as UTF-8 percent-encoded bytes
                c.to_string()
                    .as_bytes()
                    .iter()
                    .flat_map(|b| format!("%{b:02X}").chars().collect::<Vec<_>>())
                    .collect()
            }
        })
        .collect()
}

fn percent_decode_component(s: &str) -> String {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex_str) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(byte) = u8::from_str_radix(hex_str, 16) {
                    out.push(byte);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn format_duration(d: Duration) -> String {
    let secs = d.num_seconds().abs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_basic() {
        let psk = [42u8; 32];
        let expires = Utc::now() + Duration::days(7);
        let payload = InvitePayload {
            circle_id:   "8e563c41-f0ec-4225-9764-064f1fb04341".to_string(),
            psk_bytes:   psk,
            circle_name: Some("TestCircle".to_string()),
            expires_at:  expires,
            peer_addr:   Some("/ip4/1.2.3.4/tcp/9091".to_string()),
        };

        let uri = encode(&payload);
        assert!(uri.starts_with("enochian://v1/"));

        let decoded = decode(&uri).unwrap();
        assert_eq!(decoded.circle_id, payload.circle_id);
        assert_eq!(decoded.psk_bytes, psk);
        assert_eq!(decoded.circle_name.as_deref(), Some("TestCircle"));
        assert_eq!(decoded.peer_addr.as_deref(), Some("/ip4/1.2.3.4/tcp/9091"));
    }

    #[test]
    fn expiry_detected() {
        let psk = [0u8; 32];
        let payload = InvitePayload {
            circle_id:   "8e563c41-f0ec-4225-9764-064f1fb04341".to_string(),
            psk_bytes:   psk,
            circle_name: None,
            expires_at:  Utc::now() - Duration::hours(1),
            peer_addr:   None,
        };
        let uri = encode(&payload);
        let decoded = decode(&uri).unwrap();
        assert!(check_expiry(&decoded).is_err());
    }

    #[test]
    fn ttl_parsing() {
        assert_eq!(parse_ttl("7d").unwrap(), Duration::days(7));
        assert_eq!(parse_ttl("24h").unwrap(), Duration::hours(24));
        assert!(parse_ttl("bad").is_err());
    }
}
