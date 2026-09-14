//! Delivery API and stop persistence regressions; no model/provider required.
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use enoxian::{
    agent::inbox::{Inbox, Request as Work, Status},
    api,
    config::JoinPolicy,
    control::{Author, ChatMessage, RELAY_STOPS_KEY},
    daemon::DaemonState,
    mls,
    state::AppState,
};
use tower::ServiceExt;
use yrs::{Any, Map, Transact};

fn harness() -> (axum::Router, AppState, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new(
        "c".into(),
        "circle".into(),
        dir.path().into(),
        dir.path().into(),
        String::new(),
        "human".into(),
        1,
        "local".into(),
        JoinPolicy::Manual,
        "owner".into(),
        mls::new_mls_state(mls::MlsIdentity::generate("local").unwrap(), None),
    );
    let daemon = DaemonState::new();
    daemon.insert("c".into(), state.clone());
    (api::router(daemon, None), state, dir)
}

fn work(id: &str) -> Work {
    Work {
        agent: "claude".into(),
        mention_key: "claude".into(),
        task: "private request body".into(),
        message: ChatMessage {
            thread_root: None,
            id: id.into(),
            agent_id: "human".into(),
            text: "private request body".into(),
            mentions: vec!["claude".into()],
            ts: 100,
            peer_id: "remote".into(),
            attachments: vec![],
            relay: None,
            author: Author::Human,
            reply_to: None,
        },
        relay: None,
        implicit: false,
        ambient: false,
    }
}

async fn get(router: &axum::Router, url: &str) -> (StatusCode, serde_json::Value) {
    let response = router
        .clone()
        .oneshot(Request::builder().uri(url).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn status_api_is_read_only_paginated_and_does_not_expose_prompt_payloads() {
    let (router, state, _dir) = harness();
    let inbox = Inbox::open(&state.circle_dir, 100).unwrap();
    for id in ["a", "b", "c"] {
        inbox.admit(work(id), 20, 100).unwrap();
    }
    let id = inbox.entries()[0].run_id.clone();
    inbox
        .transition(&id, Status::Pending, Status::Running, None, 101)
        .unwrap();
    let (status, page) = get(&router, "/circles/c/api/chat/executions?limit=2").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["runs"][0]["message_id"], "c");
    assert_eq!(page["runs"].as_array().unwrap().len(), 2);
    assert!(!page.to_string().contains("private request body"));
    let cursor = page["next_cursor"].as_str().unwrap();
    let (_, next) = get(
        &router,
        &format!("/circles/c/api/chat/executions?before={cursor}&limit=2"),
    )
    .await;
    assert_eq!(
        next["runs"][0]["status"], "running",
        "reading must not perform restart recovery"
    );
    assert_eq!(next["runs"][0]["message_id"], "a");
    assert!(next["next_cursor"].is_null());
    assert_eq!(inbox.entries()[0].status, Status::Running);
    let (_, filtered) = get(&router, "/circles/c/api/chat/executions?message_id=b").await;
    assert_eq!(filtered["runs"].as_array().unwrap().len(), 1);
    assert_eq!(
        get(&router, "/circles/c/api/chat/executions?before=unknown")
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(&router, "/circles/missing/api/chat/executions").await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn reading_before_activation_does_not_create_an_inbox() {
    let (router, state, _dir) = harness();
    let (_, body) = get(&router, "/circles/c/api/chat/executions").await;
    assert!(body["activated_at"].is_null());
    assert!(!Inbox::path(&state.circle_dir).exists());
}

#[tokio::test]
async fn old_stop_survives_save_restore_and_does_not_expire() {
    let (_router, state, dir) = harness();
    {
        let map = state.control.get_or_insert_map(RELAY_STOPS_KEY);
        let mut txn = state.control.transact_mut();
        map.insert(&mut txn, "chain", Any::BigInt(1));
    }
    enoxian::store::control::save(dir.path(), &state.control).unwrap();
    let (_router2, restored, _dir2) = harness();
    enoxian::store::control::restore(dir.path(), &restored.control).unwrap();
    assert!(api::chat::relay_is_stopped(&restored, "chain"));
    assert!(!api::chat::relay_is_stopped(&restored, "other-chain"));
}

#[tokio::test]
async fn explicit_retry_and_cancel_preserve_attempt_history() {
    let (app, state, dir) = harness();
    let inbox = std::sync::Arc::new(Inbox::open(dir.path(), 100).unwrap());
    *state.execution_inbox.write().unwrap() = Some(std::sync::Arc::downgrade(&inbox));
    inbox.admit(work("retry-source"), 20, 100).unwrap();
    let id = inbox.entries()[0].run_id.clone();
    inbox
        .transition(&id, Status::Pending, Status::Running, None, 101)
        .unwrap();
    inbox
        .transition(
            &id,
            Status::Running,
            Status::Failed,
            Some("exit 7".into()),
            102,
        )
        .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/circles/c/api/chat/executions/{id}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"action":"retry"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let entries = inbox.entries();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].status, Status::Failed);
    let retry_id = &entries[1].run_id;
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/circles/c/api/chat/executions/{retry_id}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"action":"cancel"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(inbox.entries()[1].status, Status::Cancelled);
}

#[tokio::test]
async fn replicated_delivery_receipts_contain_no_request_body() {
    use yrs::{updates::decoder::Decode, ReadTxn, StateVector};
    let (_, mut sender, dir) = harness();
    sender.peer_id = "remote-peer".into();
    let inbox = Inbox::open(dir.path(), 100).unwrap();
    inbox.admit(work("remote-work"), 20, 100).unwrap();
    enoxian::api::execution::publish(&sender, &inbox).unwrap();
    let (app, receiver, _) = harness();
    let update = sender
        .control
        .transact()
        .encode_state_as_update_v1(&StateVector::default());
    receiver
        .control
        .transact_mut()
        .apply_update(yrs::Update::decode_v1(&update).unwrap())
        .unwrap();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/circles/c/api/chat/deliveries")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("remote-peer"));
    assert!(text.contains("pending"));
    assert!(!text.contains("private request body"));
}

#[test]
fn receipt_retention_is_bounded_and_does_not_resurrect_or_remove_remote_receipts() {
    use yrs::{ReadTxn, WriteTxn};
    let (_router, state, dir) = harness();
    let inbox = Inbox::open(dir.path(), 100).unwrap();
    let now = chrono::Utc::now().timestamp();
    {
        let mut txn = state.control.transact_mut();
        let map = txn.get_or_insert_map(api::execution::RECEIPTS_KEY);
        map.insert(
            &mut txn,
            "remote",
            serde_json::json!({"peer_id":"remote", "run_id":"remote"}).to_string(),
        );
    }
    for i in 0..110 {
        let enoxian::agent::inbox::Admission::Accepted { entry, .. } =
            inbox.admit(work(&format!("m{i}")), 20, now).unwrap()
        else {
            panic!("expected admission");
        };
        inbox
            .transition(&entry.run_id, Status::Pending, Status::Running, None, now)
            .unwrap();
        inbox
            .transition(&entry.run_id, Status::Running, Status::Completed, None, now)
            .unwrap();
        api::execution::publish(&state, &inbox).unwrap();
    }
    for _ in 0..2 {
        api::execution::publish(&state, &inbox).unwrap();
        let txn = state.control.transact();
        let map = txn.get_map(api::execution::RECEIPTS_KEY).unwrap();
        assert_eq!(map.len(&txn), 101);
        assert!(map.get(&txn, "remote").is_some());
    }
    assert_eq!(
        inbox.entries().len(),
        110,
        "local durable dedup history remains intact"
    );
}
