//! The pull side of agent engagement: listing what is waiting, and claiming a
//! message so the room's ambient listeners leave it alone.
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use enoxian::{
    api,
    config::JoinPolicy,
    control::{Author, ChatMessage, CHAT_KEY},
    daemon::DaemonState,
    mls,
    state::AppState,
};
use tower::ServiceExt;
use yrs::{Any, Array, Transact, WriteTxn};

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

fn post(state: &AppState, id: &str, text: &str, author: Author, reply_to: Option<&str>) {
    let message = ChatMessage {
        thread_root: None,
        id: id.into(),
        agent_id: if author == Author::Agent {
            "codex"
        } else {
            "suzy"
        }
        .into(),
        text: text.into(),
        mentions: vec![],
        ts: chrono::Utc::now().timestamp(),
        peer_id: "local".into(),
        attachments: vec![],
        relay: None,
        author,
        reply_to: reply_to.map(str::to_string),
    };
    let mut txn = state.control.transact_mut();
    let chat = txn.get_or_insert_array(CHAT_KEY);
    chat.push_back(
        &mut txn,
        Any::String(serde_json::to_string(&message).unwrap().into()),
    );
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: &str,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(if body.is_empty() {
                    Body::empty()
                } else {
                    Body::from(body.to_string())
                })
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

#[tokio::test]
async fn the_open_list_is_what_nobody_has_answered() {
    let (app, state, _dir) = harness();
    post(
        &state,
        "q-answered",
        "how does the retry path work?",
        Author::Human,
        None,
    );
    post(
        &state,
        "a",
        "it retries on the reconcile tick",
        Author::Agent,
        Some("q-answered"),
    );
    post(
        &state,
        "q-open",
        "and how should the backoff behave when a peer is offline?",
        Author::Human,
        None,
    );

    let (status, body) = call(&app, "GET", "/circles/c/api/inbox", "").await;
    assert_eq!(status, StatusCode::OK);
    let open: Vec<&str> = body["open"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["message_id"].as_str().unwrap())
        .collect();
    assert_eq!(
        open,
        vec!["q-open"],
        "answered and agent-authored messages are not open"
    );
}

#[tokio::test]
async fn chatter_the_room_would_never_answer_is_not_listed_as_waiting() {
    // Measured against real Circles, the unfiltered list was mostly "hi", "123"
    // and connectivity checks, which buried the one real question. The open
    // list uses the same test the room does, so the two cannot drift.
    let (app, state, _dir) = harness();
    post(&state, "hi", "hi", Author::Human, None);
    post(&state, "digits", "123", Author::Human, None);
    post(
        &state,
        "real",
        "how should the backoff behave when a peer is offline?",
        Author::Human,
        None,
    );
    let (_, body) = call(&app, "GET", "/circles/c/api/inbox", "").await;
    let open: Vec<&str> = body["open"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["message_id"].as_str().unwrap())
        .collect();
    assert_eq!(open, vec!["real"]);
}

#[tokio::test]
async fn claiming_shows_up_for_everyone_and_release_gives_it_back() {
    let (app, state, _dir) = harness();
    post(
        &state,
        "4f2a9c1e-question",
        "how should the backoff behave when a peer is offline?",
        Author::Human,
        None,
    );

    // A prefix is enough: the CLI shows eight characters, not thirty-six.
    let (status, claimed) = call(
        &app,
        "POST",
        "/circles/c/api/inbox/claim",
        r#"{"message_id":"4f2a9c1e","agent_id":"reviewer"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{claimed}");
    assert_eq!(claimed["message_id"], "4f2a9c1e-question");
    assert_eq!(claimed["claimed"], true);

    let (_, inbox) = call(&app, "GET", "/circles/c/api/inbox", "").await;
    assert_eq!(inbox["open"][0]["claimed_by"]["agent"], "reviewer");
    assert_eq!(inbox["claims"][0]["explicit"], true);

    let (status, _) = call(
        &app,
        "POST",
        "/circles/c/api/inbox/release",
        r#"{"message_id":"4f2a9c1e","agent_id":"reviewer"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, inbox) = call(&app, "GET", "/circles/c/api/inbox", "").await;
    assert!(
        inbox["open"][0]["claimed_by"].is_null(),
        "released claims hold nothing"
    );
    assert!(inbox["claims"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_claim_cannot_be_held_indefinitely() {
    // A claim silences every ambient listener in the room while it lasts. An
    // unbounded one would let a forgotten claim quietly mute a message for good.
    let (app, state, _dir) = harness();
    post(
        &state,
        "bounded-q",
        "how should the backoff behave when a peer is offline?",
        Author::Human,
        None,
    );
    let (_, claimed) = call(
        &app,
        "POST",
        "/circles/c/api/inbox/claim",
        r#"{"message_id":"bounded-q","agent_id":"reviewer","ttl_secs":999999}"#,
    )
    .await;
    let held = claimed["expires_at"].as_i64().unwrap() - chrono::Utc::now().timestamp();
    assert!(
        held <= enoxian::agent::claims::MAX_CLAIM_SECS,
        "held for {held}s"
    );
}

#[tokio::test]
async fn an_ambiguous_or_unknown_prefix_is_refused_rather_than_guessed() {
    let (app, state, _dir) = harness();
    post(
        &state,
        "abcd-one",
        "what is the first thing we should check here?",
        Author::Human,
        None,
    );
    post(
        &state,
        "abcd-two",
        "and what is the second thing we should check?",
        Author::Human,
        None,
    );
    let (status, _) = call(
        &app,
        "POST",
        "/circles/c/api/inbox/claim",
        r#"{"message_id":"abcd","agent_id":"reviewer"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "two matches: claim neither");
    let (status, _) = call(
        &app,
        "POST",
        "/circles/c/api/inbox/claim",
        r#"{"message_id":"zzzz","agent_id":"reviewer"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
