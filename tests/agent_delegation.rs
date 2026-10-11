//! End-to-end coverage for agent-to-agent delegation.
//!
//! The unit tests in `agent::relay` prove the bounds in isolation. These drive
//! the real HTTP router and the real event bus, because the property that
//! actually matters is emergent: a cascade of agent replies *stops*, and it
//! stops for the reasons the design claims rather than by accident.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use enoxian::api;
use enoxian::api::chat::Trigger;
use enoxian::config::JoinPolicy;
use enoxian::control::{ChatMessage, CircleEvent};
use enoxian::daemon::DaemonState;
use enoxian::mls;
use enoxian::state::AppState;
use tower::ServiceExt;

const CIRCLE: &str = "circle-relay";

fn harness() -> (axum::Router, AppState, tempfile::TempDir) {
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
    daemon.insert(CIRCLE.into(), state.clone());
    (api::router(daemon, None), state, dir)
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

fn post_chat(body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/circles/{CIRCLE}/api/chat"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn transcript(router: &axum::Router) -> Vec<ChatMessage> {
    let (status, body) = send(
        router,
        Request::builder()
            .uri(format!("/circles/{CIRCLE}/api/chat"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    serde_json::from_slice(&body).unwrap()
}

/// Drain the trigger events emitted so far, as `(agent, message_id)` pairs.
fn drained(rx: &mut tokio::sync::broadcast::Receiver<CircleEvent>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let CircleEvent::AgentMentioned { agent_id, message } = event {
            out.push((agent_id, message.id));
        }
    }
    out
}

#[tokio::test]
async fn human_post_mints_a_budget_and_fires_every_mention() {
    let (router, state, _dir) = harness();
    let mut rx = state.events.subscribe();

    let (status, _) = send(
        &router,
        post_chat(
            serde_json::json!({ "text": "@claude @codex look at this", "agent_id": "alice" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let fired: Vec<String> = drained(&mut rx).into_iter().map(|(a, _)| a).collect();
    assert_eq!(
        fired,
        vec!["claude", "codex"],
        "a person addressing two agents should wake both"
    );

    let msgs = transcript(&router).await;
    let relay = msgs[0].relay.as_ref().expect("human post mints a relay");
    assert_eq!(relay.spent, 0);
    assert!(relay.path.is_empty());
    assert_eq!(
        relay.root, msgs[0].id,
        "a human message roots its own cascade"
    );
    assert_eq!(relay.root_peer, "peer-local");
}

#[tokio::test]
async fn an_agent_reply_hands_off_once_and_carries_the_chain() {
    let (router, state, _dir) = harness();

    send(
        &router,
        post_chat(serde_json::json!({ "text": "@claude please start", "agent_id": "alice" })),
    )
    .await;
    let root = transcript(&router).await[0].clone();
    let mut rx = state.events.subscribe();

    // Claude replies, naming two agents. Only the first is honoured.
    enoxian::api::chat::post_message(
        &state,
        "claude".into(),
        "@codex can you check, and @gemini too".into(),
        Trigger::AgentReply {
            agent: "claude".into(),
            parent: root.relay.clone(),
        },
    )
    .unwrap();

    let fired: Vec<String> = drained(&mut rx).into_iter().map(|(a, _)| a).collect();
    assert_eq!(fired, vec!["codex"], "fan-out is one hand-off per reply");

    let reply = transcript(&router).await[1].clone();
    let relay = reply.relay.unwrap();
    assert_eq!(relay.root, root.id, "the cascade stays rooted in the human");
    assert_eq!(relay.spent, 1);
    assert_eq!(relay.path, vec!["claude"]);
    assert_eq!(
        reply.mentions,
        vec!["codex", "gemini"],
        "both mentions are still stored for rendering — only the trigger is gated"
    );
}

#[tokio::test]
async fn one_devices_agent_can_hand_off_to_the_same_name_on_another_device() {
    // Two machines each run an agent called "claude". A hand-off from one to
    // the other used to be read as a self-mention and silently never fired.
    let (router, state, _dir) = harness();
    send(
        &router,
        post_chat(serde_json::json!({ "text": "@claude say hello to the other one", "agent_id": "alice" })),
    )
    .await;
    let root = transcript(&router).await[0].clone();
    let mut rx = state.events.subscribe();

    enoxian::api::chat::post_message(
        &state,
        "claude".into(),
        "@suzy/jessair/claude — hi, give us a wave".into(),
        Trigger::AgentReply {
            agent: "claude".into(),
            parent: root.relay.clone(),
        },
    )
    .unwrap();

    let fired: Vec<String> = drained(&mut rx).into_iter().map(|(a, _)| a).collect();
    assert_eq!(
        fired,
        vec!["suzy/jessair/claude"],
        "a different machine's agent shares only a name, not an identity"
    );
}

#[tokio::test]
async fn an_agent_never_wakes_itself() {
    let (router, state, _dir) = harness();
    send(
        &router,
        post_chat(serde_json::json!({ "text": "@claude go", "agent_id": "alice" })),
    )
    .await;
    let root = transcript(&router).await[0].clone();
    let mut rx = state.events.subscribe();

    enoxian::api::chat::post_message(
        &state,
        "claude".into(),
        "I should ask @claude about this".into(),
        Trigger::AgentReply {
            agent: "claude".into(),
            parent: root.relay.clone(),
        },
    )
    .unwrap();

    assert!(
        drained(&mut rx).is_empty(),
        "a self-mention must not re-wake the agent — that is the one-agent loop"
    );
}

#[tokio::test]
async fn a_two_agent_cascade_terminates_on_its_own() {
    // The property the whole design exists for: agents left to ping-pong
    // unattended run out of budget and stop, without anyone intervening.
    let (router, state, _dir) = harness();
    send(
        &router,
        post_chat(serde_json::json!({ "text": "@claude kick it off", "agent_id": "alice" })),
    )
    .await;
    let root = transcript(&router).await[0].clone();
    let root_id = root.id.clone();

    let mut rx = state.events.subscribe();
    let mut parent = root.relay.clone();
    let mut turns = 0;
    loop {
        let (speaker, target) = if turns % 2 == 0 {
            ("claude", "codex")
        } else {
            ("codex", "claude")
        };
        enoxian::api::chat::post_message(
            &state,
            speaker.into(),
            format!("over to you @{target}"),
            Trigger::AgentReply {
                agent: speaker.into(),
                parent: parent.clone(),
            },
        )
        .unwrap();

        let fired = drained(&mut rx);
        let posted = transcript(&router).await.last().unwrap().clone();
        parent = posted.relay.clone();

        if fired.is_empty() {
            break; // budget exhausted: the mention went inert
        }
        turns += 1;
        assert!(
            turns < 500,
            "cascade never terminated — this is the runaway the budget exists to prevent"
        );
    }

    let msgs = transcript(&router).await;
    let last = msgs.last().unwrap().relay.as_ref().unwrap();
    assert_eq!(last.root, root_id, "every turn stayed in the same cascade");
    // The budget counts agent *turns*: 20 replies were posted, the 20th of
    // which found no budget left to hand off again.
    assert_eq!(
        last.spent,
        enoxian::agent::relay::DEFAULT_MAX_RELAY_TURNS,
        "the cascade stopped at the default budget, not before or after"
    );
    assert_eq!(
        turns as u8,
        enoxian::agent::relay::DEFAULT_MAX_RELAY_TURNS - 1,
        "N turns of budget buy N-1 hand-offs; anything less means another \
         bound cut the cascade short"
    );
}

#[tokio::test]
async fn a_person_can_halt_a_running_cascade() {
    let (router, state, _dir) = harness();
    send(
        &router,
        post_chat(serde_json::json!({ "text": "@claude go", "agent_id": "alice" })),
    )
    .await;
    let root = transcript(&router).await[0].clone();

    assert!(
        !enoxian::api::chat::relay_is_stopped(&state, &root.id),
        "a fresh cascade is not stopped"
    );

    let (status, _) = send(
        &router,
        Request::builder()
            .method("POST")
            .uri(format!("/circles/{CIRCLE}/api/chat/relay/stop"))
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({ "root": root.id }).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    assert!(
        enoxian::api::chat::relay_is_stopped(&state, &root.id),
        "the stop must be readable by the device that would run the next turn"
    );
    assert!(
        !enoxian::api::chat::relay_is_stopped(&state, "some-other-root"),
        "stopping one cascade must not stop every cascade"
    );
}

#[tokio::test]
async fn a_stop_needs_a_root() {
    let (router, _state, _dir) = harness();
    let (status, _) = send(
        &router,
        Request::builder()
            .method("POST")
            .uri(format!("/circles/{CIRCLE}/api/chat/relay/stop"))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::json!({ "root": "  " }).to_string()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn system_posts_carry_no_budget_and_trigger_nothing() {
    let (router, state, _dir) = harness();
    let mut rx = state.events.subscribe();

    enoxian::api::chat::post_message(
        &state,
        "system".into(),
        "@claude failed to start · adapter missing".into(),
        Trigger::System,
    )
    .unwrap();

    assert!(
        drained(&mut rx).is_empty(),
        "a failure notice that wakes the agent it is about is a loop"
    );
    assert!(
        transcript(&router).await[0].relay.is_none(),
        "a system post must not mint a budget"
    );
}

/// An agent enoxian runs can post through the CLI (`enox say`) with the token
/// it was handed, instead of in its turn reply. That post is still the agent:
/// were it a person's, it would mint a fresh budget and be offered to every
/// agent reading the room, and two agents could wake each other forever (#254).
#[tokio::test]
async fn a_managed_run_posting_through_the_api_is_the_agent_not_a_person() {
    let (router, state, _dir) = harness();
    send(
        &router,
        post_chat(serde_json::json!({ "text": "@claude please start", "agent_id": "alice" })),
    )
    .await;
    let root = transcript(&router).await[0].clone();
    let (token, _) =
        state
            .actor_tokens
            .issue_for_run(CIRCLE, "peer-local", "claude", root.relay.clone());
    let mut rx = state.events.subscribe();

    let (status, _) = send(
        &router,
        post_chat(serde_json::json!({
            "text": "@codex @gemini over to you",
            "actor_token": token,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let said = transcript(&router).await.pop().unwrap();
    assert_eq!(said.agent_id, "claude");
    assert_eq!(said.author, enoxian::control::Author::Agent);
    let relay = said.relay.expect("an agent post carries the cascade");
    assert_eq!(relay.root, root.id, "continues the cascade that woke it");
    assert_eq!(relay.spent, 1);
    assert_eq!(relay.path, vec!["claude"]);
    let fired: Vec<String> = drained(&mut rx).into_iter().map(|(a, _)| a).collect();
    assert_eq!(
        fired,
        vec!["codex"],
        "an agent hands off once, like a reply"
    );
}

/// `enox agent run` launches the agent itself and registers its token as a
/// managed run. No message woke it, so its first post starts a cascade of its
/// own, already counted as one agent turn.
#[tokio::test]
async fn an_agent_launched_by_enox_agent_run_posts_as_the_agent() {
    let (router, _state, _dir) = harness();
    let (status, body) = send(
        &router,
        Request::builder()
            .method("POST")
            .uri(format!("/circles/{CIRCLE}/api/actors/register"))
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({ "agent_id": "claude", "managed_run": true }).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let token = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    send(
        &router,
        post_chat(serde_json::json!({ "text": "done", "actor_token": token })),
    )
    .await;

    let said = transcript(&router).await.pop().unwrap();
    assert_eq!(said.author, enoxian::control::Author::Agent);
    let relay = said.relay.unwrap();
    assert_eq!(relay.root, said.id);
    assert_eq!(relay.spent, 1);
    assert_eq!(relay.path, vec!["claude"]);
}

/// A token from `enox register` is unchanged: whoever holds it posts as a
/// person, since nothing says it belongs to an agent run.
#[tokio::test]
async fn a_registered_actor_still_posts_as_a_person() {
    let (router, state, _dir) = harness();
    let (token, _) = state.actor_tokens.issue(CIRCLE, "peer-local", "harness");

    let (status, _) = send(
        &router,
        post_chat(serde_json::json!({ "text": "hello", "actor_token": token })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let said = transcript(&router).await.pop().unwrap();
    assert_eq!(said.agent_id, "harness");
    assert_eq!(said.author, enoxian::control::Author::Human);
    assert_eq!(said.relay.unwrap().spent, 0);
}
