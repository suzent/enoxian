//! Chat attachment upload and retrieval.
//!
//! Bytes live in the content-addressed blob store under the circle dir; only
//! the hash and a little metadata travel in the control doc. Peers that did not
//! originate a message fetch the blob over the live sync stream
//! (`\0blob-want/` — see [`crate::network::sync`]).
//!
//! Security posture for a path that serves user-supplied bytes:
//!
//! * The content type is **sniffed from magic bytes**, never taken from the
//!   uploader. A client that labels a payload `image/png` does not get it
//!   served as one.
//! * Only a fixed set of raster image formats is accepted. SVG is rejected
//!   outright — it is a script execution vector, not merely an image.
//! * Responses carry `nosniff` and a restrictive CSP so a blob cannot be
//!   coerced into executing in the app's origin.
//! * Dimensions are parsed from format headers rather than by decoding the
//!   image, so no decoder is exposed to hostile input.

use crate::control::Attachment;
use crate::daemon::DaemonState;
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;

/// Upload ceiling. Attachments are replicated to every peer in the circle, so
/// this is a per-circle bandwidth decision, not just a per-request one.
pub const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;

/// Formats we are willing to store and serve. Deliberately raster-only.
const ALLOWED: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

/// Identify a payload by magic bytes. Returns `None` for anything unrecognised.
pub fn sniff_mime(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if data.len() >= 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

/// Intrinsic pixel size read from the format header.
///
/// Used only to reserve layout space in the transcript, so a `None` is a
/// cosmetic loss, never an error.
pub fn image_dimensions(mime: &str, d: &[u8]) -> Option<(u32, u32)> {
    match mime {
        // IHDR is always the first chunk: width/height are big-endian u32 at 16.
        "image/png" if d.len() >= 24 => Some((
            u32::from_be_bytes(d[16..20].try_into().ok()?),
            u32::from_be_bytes(d[20..24].try_into().ok()?),
        )),
        // Logical screen descriptor, little-endian u16 at offset 6.
        "image/gif" if d.len() >= 10 => Some((
            u16::from_le_bytes(d[6..8].try_into().ok()?) as u32,
            u16::from_le_bytes(d[8..10].try_into().ok()?) as u32,
        )),
        "image/jpeg" => jpeg_dimensions(d),
        "image/webp" => webp_dimensions(d),
        _ => None,
    }
}

/// Walk JPEG marker segments to the start-of-frame, which carries the size.
fn jpeg_dimensions(d: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2; // skip SOI
    while i + 9 < d.len() {
        if d[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = d[i + 1];
        // Standalone markers carry no length field.
        if marker == 0xD8 || (0xD0..=0xD9).contains(&marker) || marker == 0x01 || marker == 0xFF {
            i += 2;
            continue;
        }
        let len = u16::from_be_bytes(d[i + 2..i + 4].try_into().ok()?) as usize;
        // SOF0..SOF15, excluding DHT (C4), JPGA (C8) and DAC (CC).
        if (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC {
            return Some((
                u16::from_be_bytes(d[i + 7..i + 9].try_into().ok()?) as u32,
                u16::from_be_bytes(d[i + 5..i + 7].try_into().ok()?) as u32,
            ));
        }
        if len < 2 {
            return None;
        }
        i += 2 + len;
    }
    None
}

fn webp_dimensions(d: &[u8]) -> Option<(u32, u32)> {
    if d.len() < 30 {
        return None;
    }
    match &d[12..16] {
        // Extended format: 24-bit little-endian, stored as (size - 1).
        b"VP8X" => {
            let w = u32::from_le_bytes([d[24], d[25], d[26], 0]) + 1;
            let h = u32::from_le_bytes([d[27], d[28], d[29], 0]) + 1;
            Some((w, h))
        }
        // Lossy: 14-bit fields in the VP8 keyframe header.
        b"VP8 " => {
            let w = u16::from_le_bytes([d[26], d[27]]) as u32 & 0x3FFF;
            let h = u16::from_le_bytes([d[28], d[29]]) as u32 & 0x3FFF;
            Some((w, h))
        }
        // Lossless: 14-bit (w-1), then 14-bit (h-1), after the 0x2F signature.
        b"VP8L" if d[20] == 0x2F => {
            let bits = u32::from_le_bytes([d[21], d[22], d[23], d[24]]);
            Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
        }
        _ => None,
    }
}

/// Strip any path structure from a client-supplied filename. The name is only
/// ever a display label and a download hint — it never touches the filesystem,
/// since storage is keyed by hash — but it is rendered, so keep it boring.
pub fn sanitize_name(raw: &str, mime: &str) -> String {
    let base = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .take(120)
        .collect::<String>();
    let base = base.trim().trim_start_matches('.').to_string();
    if base.is_empty() {
        let ext = mime.rsplit('/').next().unwrap_or("bin");
        format!("image.{ext}")
    } else {
        base
    }
}

#[derive(Deserialize)]
pub struct UploadQuery {
    pub name: Option<String>,
}

/// `POST /circles/{id}/api/chat/attachments` — raw body, returns the metadata
/// the caller should echo back when posting the message.
pub async fn upload_attachment(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Query(q): Query<UploadQuery>,
    body: Bytes,
) -> impl IntoResponse {
    let Some(state) = daemon.get(&circle_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "circle not found"})),
        )
            .into_response();
    };

    if body.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "empty upload"})),
        )
            .into_response();
    }
    if body.len() > MAX_ATTACHMENT_BYTES {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({
                "error": "attachment too large",
                "max_bytes": MAX_ATTACHMENT_BYTES,
            })),
        )
            .into_response();
    }

    let Some(mime) = sniff_mime(&body).filter(|m| ALLOWED.contains(m)) else {
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Json(json!({
                "error": "unsupported attachment type",
                "allowed": ALLOWED,
            })),
        )
            .into_response();
    };

    let blobs = match state.blobs() {
        Ok(b) => b,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": error.to_string()})),
            )
                .into_response()
        }
    };
    let hash = match blobs.put(&body) {
        Ok(hash) => hash,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": error.to_string()})),
            )
                .into_response()
        }
    };

    let (width, height) = match image_dimensions(mime, &body) {
        Some((w, h)) => (Some(w), Some(h)),
        None => (None, None),
    };

    let attachment = Attachment {
        hash,
        mime: mime.to_string(),
        name: sanitize_name(q.name.as_deref().unwrap_or(""), mime),
        size: body.len() as u64,
        width,
        height,
    };
    (StatusCode::CREATED, Json(attachment)).into_response()
}

/// `GET /circles/{id}/api/blobs/{hash}` — serve blob bytes.
///
/// On the authed API router, so this inherits the circle token middleware.
pub async fn get_blob(
    State(daemon): State<DaemonState>,
    Path((circle_id, hash)): Path<(String, String)>,
) -> impl IntoResponse {
    let Some(state) = daemon.get(&circle_id) else {
        return (StatusCode::NOT_FOUND, "circle not found").into_response();
    };
    let Ok(blobs) = state.blobs() else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "blob store unavailable").into_response();
    };
    // `get` rejects a malformed hash, so traversal cannot reach here.
    let Ok(bytes) = blobs.get(&hash) else {
        // Not necessarily missing forever: the owning peer may not have sent it
        // yet. Ask for it now so a retry succeeds.
        state.request_missing_blobs([hash.clone()]);
        return (StatusCode::NOT_FOUND, "blob not available yet").into_response();
    };

    // Re-sniff on the way out. The stored bytes are content-addressed and
    // immutable, but deriving the type here means a blob is never served as
    // something other than what it actually is.
    let mime = sniff_mime(&bytes)
        .filter(|m| ALLOWED.contains(m))
        .unwrap_or("application/octet-stream");

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            // Content-addressed: the bytes for a hash can never change.
            (
                header::CACHE_CONTROL,
                "private, max-age=31536000, immutable",
            ),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; img-src 'self'; sandbox",
            ),
        ],
        bytes,
    )
        .into_response()
}

/// Reclaim attachment blobs no retained chat message refers to.
///
/// Chat messages expire (30 days, or the message cap) but their blobs did not,
/// so every image ever posted stayed on every device for good. Reachability is
/// the transcript: a blob no surviving message names is unreachable.
///
/// Mirrors the proposal collector's two safety rules — a blob younger than
/// [`MIN_BLOB_AGE`] is never swept, because it is written before the message
/// naming it is committed, and the transcript's own retention window is what
/// keeps a blob available long enough for an absent peer to fetch it.
pub fn collect_unreferenced_blobs(state: &crate::state::AppState) -> anyhow::Result<(usize, u64)> {
    let blobs = state.blobs()?;
    let live: std::collections::BTreeSet<String> = super::chat::transcript_attachment_hashes(state)
        .into_iter()
        .collect();

    let now = std::time::SystemTime::now();
    let mut removed = 0usize;
    let mut bytes = 0u64;
    for (hash, size, modified) in blobs.list()? {
        if live.contains(&hash) {
            continue;
        }
        // An upload is stored before the message referencing it is posted, so a
        // just-uploaded blob is legitimately unreferenced for a moment.
        match now.duration_since(modified) {
            Ok(age) if age >= MIN_BLOB_AGE => {}
            // Too young, or a clock we cannot reason about.
            _ => continue,
        }
        if blobs.remove(&hash).is_ok() {
            removed += 1;
            bytes += size;
        }
    }
    Ok((removed, bytes))
}

/// A blob younger than this is never swept. Matches the proposal collector.
pub const MIN_BLOB_AGE: std::time::Duration = crate::proposal::gc::MIN_BLOB_AGE;

#[cfg(test)]
mod tests {
    use super::*;

    /// Smallest valid 1x1 PNG.
    fn png_1x1() -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
        v.extend_from_slice(&[0, 0, 0, 13]);
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&1u32.to_be_bytes());
        v.extend_from_slice(&1u32.to_be_bytes());
        v.extend_from_slice(&[8, 6, 0, 0, 0]);
        v
    }

    #[test]
    fn sniffs_known_formats() {
        assert_eq!(sniff_mime(&png_1x1()), Some("image/png"));
        assert_eq!(sniff_mime(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(sniff_mime(b"GIF89a\x03\x00\x02\x00"), Some("image/gif"));
    }

    #[test]
    fn rejects_non_images_and_svg() {
        // SVG is text, has no magic bytes, and must never be accepted.
        assert_eq!(
            sniff_mime(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
            None
        );
        assert_eq!(sniff_mime(b"#!/bin/sh\necho hi"), None);
        assert_eq!(sniff_mime(b""), None);
        // A PNG extension on non-PNG bytes does not make it a PNG: sniffing is
        // the only thing that decides.
        assert_eq!(sniff_mime(b"not really an image"), None);
    }

    #[test]
    fn reads_png_and_gif_dimensions() {
        assert_eq!(image_dimensions("image/png", &png_1x1()), Some((1, 1)));
        assert_eq!(
            image_dimensions("image/gif", b"GIF89a\x03\x00\x02\x00"),
            Some((3, 2))
        );
    }

    #[test]
    fn reads_jpeg_dimensions_from_sof() {
        // SOI, then a JFIF APP0 we must skip, then SOF0 with 5x7.
        let mut v = vec![0xFF, 0xD8];
        v.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00]);
        v.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]);
        v.extend_from_slice(&7u16.to_be_bytes()); // height
        v.extend_from_slice(&5u16.to_be_bytes()); // width
        v.extend_from_slice(&[0u8; 8]);
        assert_eq!(image_dimensions("image/jpeg", &v), Some((5, 7)));
    }

    #[test]
    fn malformed_headers_do_not_panic() {
        // Truncated inputs must yield None, never an index panic — these bytes
        // arrive from the network.
        for len in 0..40usize {
            let truncated = &png_1x1()[..len.min(png_1x1().len())];
            let _ = image_dimensions("image/png", truncated);
            let _ = image_dimensions("image/webp", truncated);
            let _ = image_dimensions("image/jpeg", truncated);
        }
        let _ = image_dimensions("image/webp", b"RIFF\0\0\0\0WEBPVP8L");
    }

    #[test]
    fn sanitizes_names() {
        assert_eq!(sanitize_name("../../etc/passwd", "image/png"), "passwd");
        assert_eq!(sanitize_name("a/b/shot.png", "image/png"), "shot.png");
        assert_eq!(sanitize_name("", "image/png"), "image.png");
        assert_eq!(sanitize_name("   ", "image/jpeg"), "image.jpeg");
        assert!(!sanitize_name("bad\nname.png", "image/png").contains('\n'));
    }
}
