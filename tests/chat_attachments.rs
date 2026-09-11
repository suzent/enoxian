//! End-to-end coverage for chat image attachments.
//!
//! Exercises the real HTTP router: upload → post → read transcript → fetch
//! bytes, plus the rejection paths that keep a byte-serving endpoint safe.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use enoxian::api;
use enoxian::config::JoinPolicy;
use enoxian::control::ChatMessage;
use enoxian::daemon::DaemonState;
use enoxian::mls;
use enoxian::state::AppState;
use tower::ServiceExt;

const CIRCLE: &str = "circle-test";

fn harness() -> (axum::Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new(
        CIRCLE.into(),
        "Circle".into(),
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
        String::new(),
        "agent".into(),
        1,
        "peer-local".into(),
        JoinPolicy::Manual,
        "owner".into(),
        mls::new_mls_state(mls::MlsIdentity::generate("peer-local").unwrap(), None),
    );
    let daemon = DaemonState::new();
    daemon.insert(CIRCLE.into(), state);
    // `None` disables the token middleware — this is the documented test mode.
    (api::router(daemon, None), dir)
}

/// A minimal but structurally valid 4x2 PNG (header only; we never decode it).
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
    v.extend_from_slice(&[0, 0, 0, 13]);
    v.extend_from_slice(b"IHDR");
    v.extend_from_slice(&width.to_be_bytes());
    v.extend_from_slice(&height.to_be_bytes());
    v.extend_from_slice(&[8, 6, 0, 0, 0]);
    v
}

async fn send(router: &axum::Router, req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let res = router.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, body)
}

fn upload(name: &str, bytes: Vec<u8>) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!(
            "/circles/{CIRCLE}/api/chat/attachments?name={name}"
        ))
        .body(Body::from(bytes))
        .unwrap()
}

fn post_json(body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/circles/{CIRCLE}/api/chat"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn image_round_trips_from_upload_to_transcript_to_bytes() {
    let (router, _dir) = harness();
    let bytes = png(4, 2);

    let (status, body) = send(&router, upload("shot.png", bytes.clone())).await;
    assert_eq!(status, StatusCode::CREATED, "upload rejected");
    let att: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let hash = att["hash"].as_str().unwrap().to_string();

    // Metadata is derived server-side from the bytes, not taken on trust.
    assert_eq!(att["mime"], "image/png");
    assert_eq!(att["width"], 4);
    assert_eq!(att["height"], 2);
    assert_eq!(att["size"], bytes.len());
    assert_eq!(att["name"], "shot.png");

    let (status, _) = send(
        &router,
        post_json(serde_json::json!({
            "text": "look at this",
            "agent_id": "tester",
            "attachments": [{ "hash": hash, "name": "shot.png" }],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // The attachment must survive the CRDT round trip.
    let (status, body) = send(
        &router,
        Request::builder()
            .uri(format!("/circles/{CIRCLE}/api/chat"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let msgs: Vec<ChatMessage> = serde_json::from_slice(&body).unwrap();
    let msg = msgs.last().expect("message stored");
    assert_eq!(msg.attachments.len(), 1);
    assert_eq!(msg.attachments[0].hash, hash);
    assert_eq!(msg.attachments[0].width, Some(4));

    // And the bytes must come back byte-identical, correctly typed and guarded.
    let (status, served) = send(
        &router,
        Request::builder()
            .uri(format!("/circles/{CIRCLE}/api/blobs/{hash}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(served, bytes, "served bytes differ from uploaded bytes");
}

#[tokio::test]
async fn blob_response_carries_hardening_headers() {
    let (router, _dir) = harness();
    let (_, body) = send(&router, upload("a.png", png(1, 1))).await;
    let att: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let hash = att["hash"].as_str().unwrap();

    let res = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/circles/{CIRCLE}/api/blobs/{hash}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let headers = res.headers();
    assert_eq!(headers["content-type"], "image/png");
    // Without nosniff a browser could reinterpret blob bytes as another type.
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert!(headers["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("default-src 'none'"));
}

#[tokio::test]
async fn rejects_non_image_uploads() {
    let (router, _dir) = harness();

    // SVG is the important one: it is an image to a user, a script to a browser.
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)"/>"#.to_vec();
    let (status, _) = send(&router, upload("x.svg", svg)).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);

    // A .png name on non-PNG bytes must not get through: sniffing decides.
    let (status, _) = send(&router, upload("evil.png", b"#!/bin/sh\nrm -rf /".to_vec())).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let (status, _) = send(&router, upload("empty.png", Vec::new())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn rejects_unknown_attachment_hash() {
    let (router, _dir) = harness();
    // A client cannot reference bytes the daemon never stored.
    let (status, _) = send(
        &router,
        post_json(serde_json::json!({
            "text": "hi",
            "agent_id": "tester",
            "attachments": [{ "hash": "a".repeat(64), "name": "ghost.png" }],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn rejects_empty_message_but_allows_image_only() {
    let (router, _dir) = harness();

    let (status, _) = send(
        &router,
        post_json(serde_json::json!({ "text": "   ", "agent_id": "tester" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "empty post should fail");

    let (_, body) = send(&router, upload("solo.png", png(2, 2))).await;
    let att: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let (status, _) = send(
        &router,
        post_json(serde_json::json!({
            "text": "",
            "agent_id": "tester",
            "attachments": [{ "hash": att["hash"], "name": "solo.png" }],
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "image-only post should succeed"
    );
}

#[tokio::test]
async fn blob_path_rejects_traversal() {
    let (router, _dir) = harness();
    for bad in ["..%2f..%2fetc%2fpasswd", "short", &"z".repeat(64)] {
        let (status, _) = send(
            &router,
            Request::builder()
                .uri(format!("/circles/{CIRCLE}/api/blobs/{bad}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_ne!(status, StatusCode::OK, "{bad} must not resolve to a blob");
    }
}

#[tokio::test]
async fn identical_images_share_one_blob() {
    let (router, dir) = harness();
    let bytes = png(3, 3);
    let (_, a) = send(&router, upload("one.png", bytes.clone())).await;
    let (_, b) = send(&router, upload("two.png", bytes)).await;
    let a: serde_json::Value = serde_json::from_slice(&a).unwrap();
    let b: serde_json::Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(a["hash"], b["hash"], "same bytes must dedupe to one blob");

    // Content addressing should mean one file on disk, not two.
    let count = walk_count(&dir.path().join("blobs"));
    assert_eq!(count, 1, "expected a single stored blob, found {count}");
}

fn walk_count(dir: &std::path::Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(|e| e.ok())
        .map(|e| {
            if e.path().is_dir() {
                walk_count(&e.path())
            } else {
                1
            }
        })
        .sum()
}
