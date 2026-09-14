//! Follow-up routing (§1.1) and the composer's view of it (§1.2), over the
//! real HTTP router.
//!
//! The unit tests in `agent::engagement` pin the resolution rule. These check
//! that the rule is wired to the endpoints the composer actually calls.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use enoxian::api;
use enoxian::api::chat::Trigger;
use enoxian::config::JoinPolicy;
use enoxian::daemon::DaemonState;
use enoxian::mls;
use enoxian::state::AppState;
use tower::ServiceExt;

const CIRCLE: &str = "circle-engagement";
const ME: &str = "peer-local";

// Run each case in a child with its own device settings. Changing process-wide
// environment from parallel async tests would race and touch the user's config.
fn isolated(name: &str, window: Option<u64>) -> bool {
    if std::env::var("ENOXIAN_ENGAGEMENT_TEST").as_deref() == Ok(name) {
        return true;
    }
    let home = tempfile::tempdir().unwrap();
    if let Some(window) = window {
        std::fs::write(
            home.path().join("agents.toml"),
            format!("engagement_window_secs = {window}\n"),
        )
        .unwrap();
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env("ENOXIAN_HOME", home.path())
        .env("ENOXIAN_ENGAGEMENT_TEST", name)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    false
}

fn harness() -> (axum::Router, AppState, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new(
        CIRCLE.into(),
        "Circle".into(),
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
        String::new(),
        "suzy-local".into(),
        1,
        ME.into(),
        JoinPolicy::Manual,
        "suzy".into(),
        mls::new_mls_state(mls::MlsIdentity::generate(ME).unwrap(), None),
    );
    let daemon = DaemonState::new();
    daemon.insert(CIRCLE.into(), state.clone());
    (api::router(daemon, None), state, dir)
}

async fn send(router: &axum::Router, req: Request<Body>) -> (StatusCode, serde_json::Value) {
    let res = router.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
    )
}

fn get(path: &str) -> Request<Body> {
    Request::builder()
        .uri(format!("/circles/{CIRCLE}/api/{path}"))
        .body(Body::empty())
        .unwrap()
}

fn post(path: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/circles/{CIRCLE}/api/{path}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// A human asks, then the agent replies — the state a follow-up depends on.
fn conversation(state: &AppState) {
    enoxian::api::chat::post_message(
        state,
        "suzy".into(),
        "@claude have a look".into(),
        Trigger::Human,
    )
    .unwrap();
    let root = state.transcript().pop().unwrap();
    enoxian::api::chat::post_message(
        state,
        "claude".into(),
        "had a look, it's fine".into(),
        Trigger::AgentReply {
            agent: "claude".into(),
            parent: root.relay.clone(),
        },
    )
    .unwrap();
}

#[tokio::test]
async fn the_composer_is_told_which_agent_a_reply_would_reach() {
    if !isolated(
        "the_composer_is_told_which_agent_a_reply_would_reach",
        Some(180),
    ) {
        return;
    }
    let (router, state, _d) = harness();
    conversation(&state);

    let (status, body) = send(&router, get("chat/engagement")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["agent"], "claude", "follow-up routes back to claude");
    assert_eq!(
        body["peer_id"], ME,
        "and to the machine that ran it, not any device configuring that name"
    );
}

#[tokio::test]
async fn there_is_no_engagement_before_an_agent_has_replied() {
    if !isolated(
        "there_is_no_engagement_before_an_agent_has_replied",
        Some(180),
    ) {
        return;
    }
    let (router, state, _d) = harness();
    enoxian::api::chat::post_message(
        state_ref(&state),
        "suzy".into(),
        "morning".into(),
        Trigger::Human,
    )
    .unwrap();

    let (_, body) = send(&router, get("chat/engagement")).await;
    assert!(body["agent"].is_null(), "nothing to follow up on: {body}");
}

fn state_ref(s: &AppState) -> &AppState {
    s
}

#[tokio::test]
async fn esc_dismisses_the_window_and_it_stays_dismissed() {
    if !isolated("esc_dismisses_the_window_and_it_stays_dismissed", Some(180)) {
        return;
    }
    let (router, state, _d) = harness();
    conversation(&state);
    assert_eq!(
        send(&router, get("chat/engagement")).await.1["agent"],
        "claude"
    );

    let (status, body) = send(&router, post("chat/engagement/exit", serde_json::json!({}))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);

    let (_, after) = send(&router, get("chat/engagement")).await;
    assert!(
        after["agent"].is_null(),
        "after Esc the next message must need a mention again: {after}"
    );
}

#[tokio::test]
async fn a_later_reply_opens_a_fresh_window_after_a_dismissal() {
    if !isolated(
        "a_later_reply_opens_a_fresh_window_after_a_dismissal",
        Some(180),
    ) {
        return;
    }
    let (router, state, _d) = harness();
    conversation(&state);
    send(&router, post("chat/engagement/exit", serde_json::json!({}))).await;
    assert!(send(&router, get("chat/engagement")).await.1["agent"].is_null());

    // Dismissing must not poison the conversation forever.
    conversation(&state);
    let (_, body) = send(&router, get("chat/engagement")).await;
    assert_eq!(body["agent"], "claude", "a new reply re-arms the window");
}

#[tokio::test]
async fn the_window_length_is_reported_so_the_composer_can_explain_itself() {
    if !isolated(
        "the_window_length_is_reported_so_the_composer_can_explain_itself",
        Some(75),
    ) {
        return;
    }
    let (router, _state, _d) = harness();
    let (_, body) = send(&router, get("chat/engagement")).await;
    assert!(
        body["window_secs"].as_i64() == Some(75),
        "the composer needs the window to describe it: {body}"
    );
}

#[tokio::test]
async fn fresh_device_requires_explicit_threads_instead_of_recency() {
    if !isolated(
        "fresh_device_requires_explicit_threads_instead_of_recency",
        None,
    ) {
        return;
    }
    let (router, state, _d) = harness();
    conversation(&state);
    let (status, body) = send(&router, get("chat/engagement")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["window_secs"], 0);
    assert!(body["agent"].is_null());
}
