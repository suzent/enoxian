use crate::control::{
    ChatActivity, ChatActivityKind, ChatMessage, CircleEvent, CHAT_ACTIVITY_KEY, CHAT_KEY,
    RELAY_STOPS_KEY,
};
use crate::daemon::DaemonState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    Json,
};
use serde::Deserialize;
use serde_json::json;
use tokio_stream::{wrappers::BroadcastStream, StreamExt};
use yrs::{Any, Array, Map, Out, ReadTxn, Transact, WriteTxn};

/// A message carries a handful of images at most; the cap keeps one post
/// from fanning out an unbounded number of blob fetches to every peer.
const MAX_ATTACHMENTS_PER_MESSAGE: usize = 10;

const TYPING_TTL_SECS: i64 = 6;
pub(crate) const AGENT_ACTIVITY_TTL_SECS: i64 = 45;

#[derive(Deserialize)]
pub struct ChatQuery {
    pub since: Option<i64>,
    pub after_id: Option<String>,
    pub limit: Option<usize>,
}

pub async fn get_chat(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Query(q): Query<ChatQuery>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "circle not found"})),
            )
                .into_response()
        }
    };
    let txn = match state.control.try_transact() {
        Ok(txn) => txn,
        Err(_) => return super::circle_busy(),
    };
    let Some(arr) = txn.get_array(CHAT_KEY) else {
        return Json(Vec::<ChatMessage>::new()).into_response();
    };
    let mut seen = std::collections::HashSet::new();
    let messages: Vec<ChatMessage> = arr
        .iter(&txn)
        .filter_map(|item| {
            if let Out::Any(Any::String(s)) = item {
                serde_json::from_str::<ChatMessage>(&s).ok()
            } else {
                None
            }
        })
        .filter(|message| seen.insert(message.id.clone()))
        .filter(|m| q.since.map(|s| m.ts > s).unwrap_or(true))
        .collect();
    let start = match q.after_id.as_deref().filter(|id| !id.is_empty()) {
        Some(id) => match messages.iter().position(|m| m.id == id) {
            Some(index) => index + 1,
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": "unknown message cursor"})),
                )
                    .into_response()
            }
        },
        None => 0,
    };
    let end = q
        .limit
        .map(|limit| (start + limit.clamp(1, 200)).min(messages.len()))
        .unwrap_or(messages.len());
    Json(&messages[start..end]).into_response()
}

#[derive(Deserialize)]
pub struct PostChatRequest {
    pub text: String,
    pub agent_id: Option<String>,
    pub actor_token: Option<String>,
    /// Blobs already uploaded via `POST .../chat/attachments`. Only the hash
    /// and display name are honoured — type, size and dimensions are re-derived
    /// from the stored bytes so a client cannot mislabel an attachment.
    #[serde(default)]
    pub attachments: Vec<AttachmentRef>,
    /// Id of the message being replied to (§1.4). Routes the turn to whichever
    /// agent posted it, with no timer and no ambiguity.
    #[serde(default)]
    pub reply_to: Option<String>,
}

#[derive(Deserialize)]
pub struct AttachmentRef {
    pub hash: String,
    #[serde(default)]
    pub name: Option<String>,
}

/// Turn client-supplied references into trustworthy [`Attachment`] metadata by
/// reading each blob back out of the local store.
fn resolve_attachments(
    state: &crate::state::AppState,
    refs: &[AttachmentRef],
) -> Result<Vec<crate::control::Attachment>, String> {
    if refs.is_empty() {
        return Ok(Vec::new());
    }
    if refs.len() > MAX_ATTACHMENTS_PER_MESSAGE {
        return Err(format!(
            "too many attachments (max {MAX_ATTACHMENTS_PER_MESSAGE})"
        ));
    }
    let blobs = state.blobs().map_err(|e| e.to_string())?;
    refs.iter()
        .map(|r| {
            let bytes = blobs
                .get(&r.hash)
                .map_err(|_| format!("unknown attachment {}", r.hash))?;
            let mime = super::attachments::sniff_mime(&bytes)
                .ok_or_else(|| format!("unsupported attachment {}", r.hash))?;
            let (width, height) = match super::attachments::image_dimensions(mime, &bytes) {
                Some((w, h)) => (Some(w), Some(h)),
                None => (None, None),
            };
            Ok(crate::control::Attachment {
                hash: r.hash.clone(),
                mime: mime.to_string(),
                name: super::attachments::sanitize_name(r.name.as_deref().unwrap_or(""), mime),
                size: bytes.len() as u64,
                width,
                height,
            })
        })
        .collect()
}

pub async fn post_chat(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Json(req): Json<PostChatRequest>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "circle not found"})),
            )
                .into_response()
        }
    };

    let actor = match super::actor::resolve_actor(
        &state,
        req.actor_token.as_deref(),
        req.agent_id,
        "unknown",
    ) {
        Ok(actor) => actor,
        Err(error) => return error.into_response(),
    };
    let sender = actor.agent_id;
    let attachments = match resolve_attachments(&state, &req.attachments) {
        Ok(a) => a,
        Err(error) => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response()
        }
    };
    // An attachment-only post is legitimate, but a post with neither text nor
    // attachment is not — it would render as an empty bubble.
    if req.text.trim().is_empty() && attachments.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "message must have text or an attachment"})),
        )
            .into_response();
    }
    // A user/UI post fires mention triggers.
    match post_reply(
        &state,
        sender,
        req.text,
        attachments,
        Trigger::Human,
        req.reply_to,
    ) {
        Ok(id) => (StatusCode::CREATED, Json(json!({ "id": id }))).into_response(),
        Err(error) if error.to_string().contains("state busy") => super::circle_busy(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": error.to_string()})),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct PostActivityRequest {
    pub actor_id: String,
    pub typing: bool,
}

/// Return only live activity. The CRDT map may retain an expired value per
/// producer so that a disconnected peer cannot leave a permanent indicator.
pub async fn get_activity(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
) -> impl IntoResponse {
    let Some(state) = daemon.get(&circle_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "circle not found"})),
        )
            .into_response();
    };
    match live_activities(&state, chrono::Utc::now().timestamp()) {
        Some(activity) => Json(activity).into_response(),
        None => super::circle_busy(),
    }
}

/// Browser-originated activity is deliberately limited to `typing`; agent
/// lifecycle states are written internally only after a mention is accepted.
pub async fn post_activity(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Json(req): Json<PostActivityRequest>,
) -> impl IntoResponse {
    let Some(state) = daemon.get(&circle_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "circle not found"})),
        )
            .into_response();
    };
    let actor_id = req.actor_id.trim();
    if actor_id.is_empty() || actor_id.len() > 200 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid actor_id"})),
        )
            .into_response();
    }

    let now = chrono::Utc::now().timestamp();
    let expires_at = if req.typing {
        now + TYPING_TTL_SECS
    } else {
        now - 1
    };
    let activity = ChatActivity {
        activity_id: format!("typing:{actor_id}"),
        actor_id: actor_id.to_string(),
        peer_id: state.peer_id.clone(),
        kind: ChatActivityKind::Typing,
        detail: None,
        message_id: None,
        updated_at: now,
        expires_at,
    };
    match put_activity(&state, activity) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(error) if error.to_string().contains("state busy") => super::circle_busy(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": error.to_string()})),
        )
            .into_response(),
    }
}

pub(crate) fn put_activity(
    state: &crate::state::AppState,
    activity: ChatActivity,
) -> anyhow::Result<()> {
    let raw = serde_json::to_string(&activity)?;
    let mut txn = state
        .control
        .try_transact_mut()
        .map_err(|_| anyhow::anyhow!("circle state busy"))?;
    let map = txn.get_or_insert_map(CHAT_ACTIVITY_KEY);
    let expired = map
        .iter(&txn)
        .filter_map(|(key, value)| match value {
            Out::Any(Any::String(raw)) => serde_json::from_str::<ChatActivity>(&raw)
                .ok()
                .filter(|item| !activity_is_live(item, chrono::Utc::now().timestamp()))
                .map(|_| key.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    for key in expired {
        map.remove(&mut txn, key.as_str());
    }
    map.insert(
        &mut txn,
        activity.activity_id.as_str(),
        Any::String(raw.as_str().into()),
    );
    drop(txn);
    let _ = state
        .events
        .send(CircleEvent::ChatActivityChanged { activity });
    Ok(())
}

fn live_activities(state: &crate::state::AppState, now: i64) -> Option<Vec<ChatActivity>> {
    let txn = state.control.try_transact().ok()?;
    let Some(map) = txn.get_map(CHAT_ACTIVITY_KEY) else {
        return Some(Vec::new());
    };
    Some(
        map.iter(&txn)
            .filter_map(|(_, value)| match value {
                Out::Any(Any::String(raw)) => serde_json::from_str::<ChatActivity>(&raw).ok(),
                _ => None,
            })
            .filter(|activity| activity_is_live(activity, now))
            .collect(),
    )
}

fn activity_is_live(activity: &ChatActivity, now: i64) -> bool {
    activity.expires_at > now
}

/// Who is posting, and with what authority to wake other agents.
///
/// This replaces the old `fire_mentions: bool`. The flag answered "may this
/// post trigger anything?" with a hard yes/no, and agent replies were always
/// `false` because an agent that mentions another agent would otherwise set
/// off an endless trigger loop. Delegation needs a middle answer: an agent
/// reply may trigger, but only within the budget its cascade still has (see
/// [`crate::agent::relay`] and `docs/guide/agents.md`).
///
/// Mentions are always *stored* on the message, for chip rendering, whatever
/// the trigger decision is.
pub enum Trigger {
    /// A person posting through the UI or CLI. Mints a fresh relay budget and
    /// fires every mention.
    Human,
    /// An agent's own reply, continuing the cascade that woke it. Fires at
    /// most one mention, never itself, and only while the budget holds.
    AgentReply {
        agent: String,
        /// The relay carried by the message that triggered this agent.
        parent: Option<crate::control::Relay>,
    },
    /// A `system` post. Never triggers anything; a failure notice that wakes
    /// an agent is a loop waiting to happen.
    System,
}

/// Post a chat message into the circle's control CRDT.
///
/// Returns the new message id.
pub fn post_message(
    state: &crate::state::AppState,
    sender: String,
    text: String,
    trigger: Trigger,
) -> anyhow::Result<String> {
    post_message_with_attachments(state, sender, text, Vec::new(), trigger)
}

/// As [`post_message_with_attachments`], but threading the post as a reply to
/// an earlier message (§1.4).
pub fn post_reply(
    state: &crate::state::AppState,
    sender: String,
    text: String,
    attachments: Vec<crate::control::Attachment>,
    trigger: Trigger,
    reply_to: Option<String>,
) -> anyhow::Result<String> {
    post_inner(state, sender, text, attachments, trigger, reply_to)
}

/// As [`post_message`], but carries attachment metadata. The bytes must already
/// be in the local blob store — only the reference travels in the control doc.
pub fn post_message_with_attachments(
    state: &crate::state::AppState,
    sender: String,
    text: String,
    attachments: Vec<crate::control::Attachment>,
    trigger: Trigger,
) -> anyhow::Result<String> {
    post_inner(state, sender, text, attachments, trigger, None)
}

fn post_inner(
    state: &crate::state::AppState,
    sender: String,
    text: String,
    attachments: Vec<crate::control::Attachment>,
    trigger: Trigger,
    reply_to: Option<String>,
) -> anyhow::Result<String> {
    let mentions = crate::agent::mention::extract(&text);
    let id = uuid::Uuid::new_v4().to_string();
    // A human post roots a new cascade at itself; an agent reply extends the
    // one that woke it. A system post carries none, so nothing downstream can
    // spend a budget on its behalf.
    let author = match &trigger {
        Trigger::Human => crate::control::Author::Human,
        Trigger::AgentReply { .. } => crate::control::Author::Agent,
        Trigger::System => crate::control::Author::System,
    };
    let relay = match &trigger {
        Trigger::Human => Some(crate::agent::relay::mint(&id, &state.peer_id)),
        Trigger::AgentReply { agent, parent } => Some(crate::agent::relay::extend(
            parent
                .as_ref()
                // An agent woken by a peer that predates the field has no
                // parent chain. Root the cascade at the message it replies to
                // rather than handing it an unbounded one.
                .unwrap_or(&crate::agent::relay::mint(&id, &state.peer_id)),
            agent,
        )),
        Trigger::System => None,
    };
    let thread_root = reply_to
        .as_ref()
        .map(|parent| {
            state
                .transcript()
                .iter()
                .find(|m| &m.id == parent)
                .and_then(|m| m.thread_root.clone())
                .unwrap_or_else(|| parent.clone())
        })
        .or_else(|| Some(id.clone()));
    let msg = ChatMessage {
        thread_root,
        id,
        agent_id: sender,
        text,
        mentions: mentions.clone(),
        ts: chrono::Utc::now().timestamp(),
        peer_id: state.peer_id.clone(),
        attachments,
        relay,
        author,
        reply_to,
    };

    let json_str = serde_json::to_string(&msg)?;
    {
        let mut txn = state
            .control
            .try_transact_mut()
            .map_err(|_| anyhow::anyhow!("circle state busy"))?;
        let arr = txn.get_or_insert_array(CHAT_KEY);
        arr.push_back(&mut txn, Any::String(json_str.as_str().into()));
    }

    let _ = state.events.send(CircleEvent::MessagePosted {
        message: msg.clone(),
    });
    // Which of those mentions actually wake anything depends on who posted.
    // A system post never triggers: `@claude failed to start` naming the agent
    // it is about would wake that agent, which is a loop. An absent relay
    // cannot stand in for this — a message from a peer predating the field has
    // no relay either, and those must still fire.
    //
    // This is the sender-side filter only; the device that would run the agent
    // re-checks the budget against its own config, which is the real gate.
    let to_fire = match &trigger {
        Trigger::System => Vec::new(),
        Trigger::Human => mentions.clone(),
        Trigger::AgentReply { .. } => {
            // This device's own labels, so a mention scoped *here* reads as
            // self while one scoped elsewhere does not.
            let me = state.self_member();
            let self_device = me
                .as_ref()
                .map(|m| (m.owner.as_str(), m.device_label.as_str()));
            crate::agent::relay::triggerable_mentions(
                &msg,
                crate::agent::relay::DEFAULT_MAX_RELAY_TURNS,
                self_device,
            )
        }
    };
    for mentioned in to_fire {
        let _ = state.events.send(CircleEvent::AgentMentioned {
            agent_id: mentioned,
            message: msg.clone(),
        });
    }

    Ok(msg.id)
}

/// Every attachment hash referenced anywhere in the transcript.
///
/// Used to backfill a late joiner: a want broadcast while no peer was connected
/// reaches nobody, so a freshly established sync stream re-asks for whatever is
/// still missing.
pub fn transcript_attachment_hashes(state: &crate::state::AppState) -> Vec<String> {
    let Ok(txn) = state.control.try_transact() else {
        return Vec::new();
    };
    let Some(arr) = txn.get_array(CHAT_KEY) else {
        return Vec::new();
    };
    arr.iter(&txn)
        .filter_map(|item| match item {
            Out::Any(Any::String(s)) => serde_json::from_str::<ChatMessage>(&s).ok(),
            _ => None,
        })
        .flat_map(|m| m.attachments.into_iter().map(|a| a.hash))
        .collect()
}

pub async fn chat_stream(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "circle not found"})),
            )
                .into_response()
        }
    };
    let rx = state.events.subscribe();
    let shutdown = daemon.shutdown_token.clone();
    let stream = BroadcastStream::new(rx).filter_map(|result| {
        result.ok().and_then(|ev| {
            // Chat messages/mentions drive the transcript; roster events let the
            // mention picker re-fetch members/presence live (e.g. when an agent
            // is added to a device's config). The frontend ignores any type it
            // doesn't handle, so forwarding these is safe.
            matches!(
                ev,
                CircleEvent::MessagePosted { .. }
                    | CircleEvent::AgentMentioned { .. }
                    | CircleEvent::ChatActivityChanged { .. }
                    | CircleEvent::AttachmentAvailable { .. }
                    | CircleEvent::MemberAdded { .. }
                    | CircleEvent::MemberRemoved { .. }
                    | CircleEvent::PresenceChanged { .. }
            )
            .then(|| serde_json::to_string(&ev).ok())
            .flatten()
            .map(|data| Ok::<_, std::convert::Infallible>(Event::default().data(data)))
        })
    });
    let stream = futures::StreamExt::take_until(stream, shutdown.cancelled_owned());
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::JoinPolicy, mls, state::AppState};
    use std::path::PathBuf;

    fn test_state(peer_id: &str) -> AppState {
        AppState::new(
            "circle".into(),
            "Circle".into(),
            PathBuf::new(),
            PathBuf::new(),
            String::new(),
            "agent".into(),
            1,
            peer_id.into(),
            JoinPolicy::Manual,
            "owner".into(),
            mls::new_mls_state(mls::MlsIdentity::generate(peer_id).unwrap(), None),
        )
    }

    // An agent reply posts under the bare agent name, which several devices may
    // configure. Without the posting peer stamped on the message, a reader can
    // only guess which device ran it — and guessing by name misattributes the
    // run to whichever member is listed first.
    #[test]
    fn posted_message_records_the_posting_peer() {
        let state = test_state("peer-macbook");
        post_message(
            &state,
            "codex".to_string(),
            "done".to_string(),
            Trigger::System,
        )
        .unwrap();

        let txn = state.control.transact();
        let arr = txn.get_array(CHAT_KEY).unwrap();
        let stored: Vec<ChatMessage> = arr
            .iter(&txn)
            .filter_map(|item| match item {
                Out::Any(Any::String(s)) => serde_json::from_str::<ChatMessage>(&s).ok(),
                _ => None,
            })
            .collect();

        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].agent_id, "codex");
        assert_eq!(stored[0].peer_id, "peer-macbook");
    }

    // Messages written before `peer_id` existed must still load; readers fall
    // back to name matching for those.
    #[test]
    fn message_without_peer_id_still_deserializes() {
        let legacy = r#"{"id":"m1","agent_id":"codex","text":"hi","mentions":[],"ts":10}"#;
        let msg: ChatMessage = serde_json::from_str(legacy).unwrap();
        assert_eq!(msg.peer_id, "");
    }

    fn activity(expires_at: i64) -> ChatActivity {
        ChatActivity {
            activity_id: "typing:alice".to_string(),
            actor_id: "alice".to_string(),
            peer_id: "peer-alice".to_string(),
            kind: ChatActivityKind::Typing,
            detail: None,
            message_id: None,
            updated_at: 10,
            expires_at,
        }
    }

    #[test]
    fn activity_expires_at_lease_boundary() {
        assert!(activity_is_live(&activity(11), 10));
        assert!(!activity_is_live(&activity(10), 10));
        assert!(!activity_is_live(&activity(9), 10));
    }

    #[test]
    fn activity_event_has_stable_wire_shape() {
        let event = CircleEvent::ChatActivityChanged {
            activity: activity(16),
        };
        let value = serde_json::to_value(event).unwrap();
        assert_eq!(value["type"], "chat_activity_changed");
        assert_eq!(value["activity"]["kind"], "typing");
        assert_eq!(value["activity"]["actor_id"], "alice");
    }
}

// ── Follow-up engagement ─────────────────────────────────────────────────────

/// The follow-up window for the caller's own device.
///
/// The composer uses this to say what will happen to the next message typed —
/// implicit routing that is invisible is a bug (§1.2).
pub async fn get_engagement(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "circle not found"})),
            )
                .into_response()
        }
    };
    let cfg = crate::agent::config::AgentConfig::load();
    let history = state.transcript();
    let engagement = crate::agent::engagement::resolve(
        &history,
        &state.peer_id,
        chrono::Utc::now().timestamp(),
        cfg.engagement_window_secs,
        state.engagement_dismissed(&state.peer_id).as_ref(),
    );
    match engagement {
        Some(e) => Json(json!({
            "agent": e.agent,
            "peer_id": e.peer_id,
            "message_id": e.message_id,
            "window_secs": cfg.engagement_window_secs,
        }))
        .into_response(),
        None => Json(json!({ "agent": null, "window_secs": cfg.engagement_window_secs }))
            .into_response(),
    }
}

/// Dismiss the follow-up window — the composer's Esc.
///
/// Recorded in the synced control doc rather than locally, because the device
/// that would route the follow-up is not necessarily this one; the agent may be
/// running on another machine.
pub async fn exit_engagement(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "circle not found"})),
            )
                .into_response()
        }
    };
    // Record *which* reply was on screen, not just when. See `Dismissal`.
    let cfg = crate::agent::config::AgentConfig::load();
    let current = crate::agent::engagement::resolve(
        &state.transcript(),
        &state.peer_id,
        chrono::Utc::now().timestamp(),
        cfg.engagement_window_secs,
        state.engagement_dismissed(&state.peer_id).as_ref(),
    );
    match mark_engagement_dismissed(&state, current.map(|e| e.message_id).unwrap_or_default()) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(error) if error.to_string().contains("state busy") => super::circle_busy(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": error.to_string()})),
        )
            .into_response(),
    }
}

fn mark_engagement_dismissed(
    state: &crate::state::AppState,
    message_id: String,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now().timestamp();
    let value = json!({ "at": now, "message_id": message_id }).to_string();
    let mut txn = state
        .control
        .try_transact_mut()
        .map_err(|_| anyhow::anyhow!("circle state busy"))?;
    let map = txn.get_or_insert_map(crate::control::ENGAGEMENT_EXITS_KEY);
    map.insert(
        &mut txn,
        state.peer_id.as_str(),
        Any::String(value.as_str().into()),
    );
    drop(txn);
    let _ = state.events.send(CircleEvent::EngagementChanged {
        peer_id: state.peer_id.clone(),
    });
    Ok(())
}

// ── Cascade stops ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct StopRelayRequest {
    /// The cascade to halt: the id of the human message that started it. A
    /// client reads it off any message in the cascade (`relay.root`).
    pub root: String,
}

/// Halt a delegation cascade.
///
/// This is the runaway kill switch. It does not interrupt a turn already in
/// flight — enoxian has no control that does — but it stops every *further*
/// turn, on every device, which is what actually bounds the spend.
///
/// Writing it into the synced control doc is the point: the device that would
/// run the next turn is usually not this one, so a local flag would stop
/// nothing. Anyone in the Circle may stop a cascade. That is deliberate — the
/// people paying attention are not always the person who started it, and the
/// worst a wrongful stop costs is a re-mention.
pub async fn stop_relay(
    State(daemon): State<DaemonState>,
    Path(circle_id): Path<String>,
    Json(req): Json<StopRelayRequest>,
) -> impl IntoResponse {
    let state = match daemon.get(&circle_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "circle not found"})),
            )
                .into_response()
        }
    };
    if req.root.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "root is required"})),
        )
            .into_response();
    }
    match mark_relay_stopped(&state, &req.root) {
        Ok(()) => Json(json!({"ok": true, "root": req.root})).into_response(),
        Err(error) if error.to_string().contains("state busy") => super::circle_busy(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": error.to_string()})),
        )
            .into_response(),
    }
}

pub(crate) fn mark_relay_stopped(state: &crate::state::AppState, root: &str) -> anyhow::Result<()> {
    let now = chrono::Utc::now().timestamp();
    let mut txn = state
        .control
        .try_transact_mut()
        .map_err(|_| anyhow::anyhow!("circle state busy"))?;
    let map = txn.get_or_insert_map(RELAY_STOPS_KEY);
    map.insert(&mut txn, root, Any::BigInt(now));
    drop(txn);
    // A stopped root may have pending work on an offline peer. Keep its
    // tombstone durable; elapsed time cannot make that work safe to launch.
    crate::store::control::save(&state.circle_dir, &state.control)?;
    let _ = state.events.send(CircleEvent::RelayStopped {
        root: root.to_string(),
    });
    Ok(())
}

/// Has anyone in the Circle halted this cascade?
///
/// Read on the receiving device before each relayed turn, so a stop from any
/// peer is honoured wherever the next turn would have run.
pub fn relay_is_stopped(state: &crate::state::AppState, root: &str) -> bool {
    try_relay_is_stopped(state, root).unwrap_or_else(|error| {
        tracing::warn!("[agent] stop state unavailable for {root}: {error}");
        false
    })
}

/// Execution can defer a queued turn when the control doc is busy rather than
/// confusing "unknown" with either a cancellation or permission to launch.
pub(crate) fn try_relay_is_stopped(
    state: &crate::state::AppState,
    root: &str,
) -> anyhow::Result<bool> {
    let txn = state
        .control
        .try_transact()
        .map_err(|_| anyhow::anyhow!("circle state busy"))?;
    let Some(map) = txn.get_map(RELAY_STOPS_KEY) else {
        return Ok(false);
    };
    Ok(matches!(
        map.get(&txn, root),
        Some(Out::Any(Any::BigInt(_)))
    ))
}
