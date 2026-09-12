//! Local reaction loop: turn chat mentions into agent runs, per this device's
//! own policy.
//!
//! The network side is just chat. An `@mention` is an ordinary replicated
//! message; no remote member can *command* execution here. This loop is the
//! per-device reaction over that stream:
//!
//! - **pull** (default): do nothing. An agent is expected to read chat and
//!   self-trigger. The loop still runs but launches nothing.
//! - **push**: when a mention names an agent in this device's allowlist,
//!   launch it through the local execution layer (argv or ACP).
//!
//! The allowlist (`agents.toml`) is the gate: a mention of an agent this device
//! does not configure is ignored. Files produced by an allowed run are already
//! live in the workspace and are recorded as accepted, revertible history.

use super::config::{AcceptFrom, AgentConfig, Reaction};
use super::driver::{self, Initiator};
use super::mention::Mention;
use crate::control::{ChatActivity, ChatActivityKind, CircleEvent, Relay};
use crate::proposal::store::ProposalStore;
use crate::state::AppState;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

pub fn spawn_reaction(state: AppState, token: CancellationToken) {
    tokio::spawn(async move {
        if let Err(e) = run(state, token).await {
            tracing::warn!("[agent] reaction loop stopped: {e:#}");
        }
    });
}

async fn run(state: AppState, token: CancellationToken) -> anyhow::Result<()> {
    let mut events = state.events.subscribe();

    // Durable dedup: act on each (message, mention) at most once *ever*, not
    // just once per run. On reconnect, P2P sync replays the whole chat history
    // as fresh CRDT updates and the observer fires AgentMentioned for every
    // historical message; without a persisted guard, every past mention would
    // re-launch its agent on each restart. This survives restarts.
    let handled = super::handled::HandledMentions::load(&state.circle_dir);
    let ledger = std::sync::Arc::new(RelayLedger::default());
    let queue = RunQueue::new();
    spawn_run_worker(state.clone(), queue.clone(), token.clone());

    // Cheap first-line filter: a mention older than daemon start is almost
    // certainly replayed history. The durable set is the real guard; this just
    // avoids logging/looking up ancient messages. Grace of 2s for clock skew.
    let cutoff = chrono::Utc::now().timestamp() - 2;
    tracing::info!(
        "[agent] reaction loop started for circle {} (fresh cutoff ts={cutoff})",
        state.circle_id
    );

    loop {
        tokio::select! {
            _ = token.cancelled() => break,
            evt = events.recv() => match evt {
                // A message with no agent mention may still be a follow-up to
                // the agent that just replied to this speaker (§1.1).
                Ok(CircleEvent::MessagePosted { message }) => {
                    if message.ts < cutoff {
                        continue;
                    }
                    let cfg = AgentConfig::load();
                    offer_ambient(&state, &handled, &ledger, &queue, &cfg, &message);
                    let Some(engagement) = resolve_followup(&state, &message, &cfg) else {
                        continue;
                    };
                    // The whole text is the task: there is no mention prefix to
                    // strip.
                    dispatch(
                        &state,
                        &handled,
                        &ledger,
                        &queue,
                        &cfg,
                        DispatchRequest {
                            agent: &engagement.agent,
                            mention_key: &super::engagement::dedup_key(&engagement.agent),
                            task: message.text.clone(),
                            message: &message,
                            relay: Some(super::engagement::followup_relay(
                                &message.id,
                                &message.peer_id,
                            )),
                            implicit: true,
                            ambient: false,
                        },
                    );
                }
                Ok(CircleEvent::AgentMentioned { agent_id, message }) => {
                    // Old message (replayed history) — skip cheaply.
                    if message.ts < cutoff {
                        continue;
                    }
                    // `agent_id` is the stored mention body — possibly scoped as
                    // owner/device/agent. Only agent-level targets launch; user-
                    // and device-level mentions are notify-only.
                    let Some(mention) = Mention::parse(&agent_id) else { continue };
                    let Some((agent, scope)) = mention.agent_target() else {
                        tracing::debug!("[agent] `{agent_id}` is a notify-only mention — not launching");
                        continue;
                    };

                    // If the mention is scoped to a specific device, only that
                    // device reacts. A device never runs an agent addressed to a
                    // different device.
                    if let Some((owner, device)) = scope {
                        if !targets_this_device(&state, owner, device) {
                            continue;
                        }
                    }

                    // Reload config per mention so edits to agents.toml take
                    // effect without a daemon restart — mentions are rare.
                    let cfg = AgentConfig::load();
                    dispatch(
                        &state,
                        &handled,
                        &ledger,
                        &queue,
                        &cfg,
                        DispatchRequest {
                            agent,
                            mention_key: &agent_id,
                            task: strip_mention(&message.text, &agent_id),
                            message: &message,
                            relay: message.relay.clone(),
                            implicit: false,
                            ambient: false,
                        },
                    );
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("[agent] reaction stream lagged by {n}; some mentions dropped");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
    Ok(())
}

/// Everything needed to decide whether a turn runs, from either entry path:
/// an explicit mention, or a follow-up routed by the engagement window.
struct DispatchRequest<'a> {
    /// The resolved agent name.
    agent: &'a str,
    /// Durable-dedup key. The mention body for an explicit mention; a
    /// synthetic `@agent` for a follow-up, which has no mention to key on.
    mention_key: &'a str,
    task: String,
    message: &'a crate::control::ChatMessage,
    relay: Option<Relay>,
    /// True when routed by the engagement window rather than addressed.
    implicit: bool,
    /// True when nobody addressed this agent at all — it is being offered the
    /// room's conversation and may decline (§2).
    ambient: bool,
}

/// The shared gate: allowlist, delegation budget, dedup, then enqueue.
fn dispatch(
    state: &AppState,
    handled: &super::handled::HandledMentions,
    ledger: &RelayLedger,
    queue: &RunQueue,
    cfg: &AgentConfig,
    req: DispatchRequest<'_>,
) {
    if cfg.reaction != Reaction::Push {
        tracing::debug!("[agent] pull policy — ignoring `{}`", req.agent);
        return;
    }
    let Some(cmd) = cfg.resolve(req.agent).cloned() else {
        // Not one of this device's agents — nothing to do.
        return;
    };

    // Delegation gate. Only an agent-authored trigger is budgeted; a follow-up
    // is a human's message and mints its own (§3.3).
    if !req.implicit && !req.ambient {
        let delegated = req
            .relay
            .as_ref()
            .and_then(|r| super::relay::poster(r))
            .map(str::to_string);
        if let Some(via) = &delegated {
            if cmd.accept_from != AcceptFrom::Agents {
                tracing::debug!(
                    "[agent] `{}` does not accept delegation (mentioned by `{via}`) — skipping",
                    req.agent
                );
                return;
            }
            let relay = req.relay.as_ref().expect("delegated implies a relay");
            if crate::api::chat::relay_is_stopped(state, &relay.root) {
                tracing::info!(
                    "[agent] cascade {} was stopped — not waking `{}`",
                    relay.root,
                    req.agent
                );
                publish_relay_skipped(state, req.agent, &req.message.id, "cascade stopped");
                return;
            }
            if !super::relay::has_budget(relay, cmd.max_relay_turns) {
                tracing::info!(
                    "[agent] relay budget spent on cascade {} — not waking `{}`",
                    relay.root,
                    req.agent
                );
                publish_relay_skipped(state, req.agent, &req.message.id, "relay budget spent");
                return;
            }
            if !ledger.charge(&relay.root, cmd.max_relay_turns) {
                tracing::info!(
                    "[agent] cascade {} has spent this device's budget — not waking `{}`",
                    relay.root,
                    req.agent
                );
                publish_relay_skipped(state, req.agent, &req.message.id, "relay budget spent");
                return;
            }
        }
    }

    // Only durable-dedup turns that passed every check and are about to run.
    if !handled.mark_new(&req.message.id, req.mention_key) {
        tracing::debug!(
            "[agent] {}::{} already handled — skipping",
            req.message.id,
            req.mention_key
        );
        return;
    }

    if req.implicit {
        tracing::info!(
            "[agent] routing a follow-up to `{}` (no mention needed)",
            req.agent
        );
    }
    publish_agent_activity(
        state,
        req.agent,
        &req.message.id,
        ChatActivityKind::Seen,
        true,
    );

    // Sender-origin sets the acceptance-policy posture. For a relayed turn this
    // resolves from the *root human*, not the agent that mentioned us —
    // otherwise an agent could launder a remote member's request into a local
    // one by relaying it.
    let initiator = if req.ambient {
        // Nobody asked. Whatever it writes is held for review (§2.4).
        Initiator::Ambient
    } else if attributed_local(state, req.message) {
        Initiator::Local
    } else {
        Initiator::RemoteMember
    };

    let agent_id = req.agent.to_string();
    let dropped = queue.push(QueuedTurn {
        agent_id: agent_id.clone(),
        cmd,
        task: req.task,
        sender: req.message.agent_id.clone(),
        message_id: req.message.id.clone(),
        initiator,
        relay: req.relay,
        ambient: req.ambient,
    });
    if let Some(dropped_id) = dropped {
        // Dropping a message the user wrote is lossy, and rare by construction
        // — so unlike a queued turn, it is worth a line in the transcript.
        tracing::warn!(
            "[agent] queue for `{agent_id}` is full — dropped the oldest turn ({dropped_id})"
        );
        let _ = crate::api::chat::post_message(
            state,
            "system".to_string(),
            format!(
                "@{agent_id} has {MAX_QUEUED_PER_AGENT} messages waiting — the oldest was dropped"
            ),
            crate::api::chat::Trigger::System,
        );
    }
}

/// Offer an unaddressed message to this device's ambient agents (§2).
fn offer_ambient(
    state: &AppState,
    handled: &super::handled::HandledMentions,
    ledger: &RelayLedger,
    queue: &RunQueue,
    cfg: &AgentConfig,
    message: &crate::control::ChatMessage,
) {
    let ambient: Vec<String> = cfg
        .agents
        .iter()
        .filter(|(_, cmd)| cmd.is_ambient())
        .map(|(name, _)| name.clone())
        .collect();
    if ambient.is_empty() {
        return;
    }
    if let Some(reason) = super::ambient::skip_reason(message, mentions_an_agent(message)) {
        tracing::trace!("[agent] no ambient turn for {}: {reason}", message.id);
        return;
    }
    let history = state.transcript();
    let mut offered = 0;
    for agent in ambient {
        if offered >= super::ambient::MAX_AMBIENT_REPLIES_PER_MESSAGE {
            tracing::debug!(
                "[agent] ambient cap reached for {} — `{agent}` not offered",
                message.id
            );
            break;
        }
        if super::ambient::spoke_recently(&history, &agent, message.ts) {
            continue;
        }
        dispatch(
            state,
            handled,
            ledger,
            queue,
            cfg,
            DispatchRequest {
                agent: &agent,
                mention_key: &format!("~ambient:{agent}"),
                task: message.text.clone(),
                message,
                // An ambient turn is rooted in the human message it reads, so a
                // hand-off from it spends that message's budget (§3.8).
                relay: Some(super::relay::mint(&message.id, &message.peer_id)),
                implicit: false,
                ambient: true,
            },
        );
        offered += 1;
    }
}

/// Does this message address an agent (rather than a person or nobody)?
fn mentions_an_agent(message: &crate::control::ChatMessage) -> bool {
    message.mentions.iter().any(|m| {
        Mention::parse(m)
            .and_then(|parsed| parsed.agent_target().map(|_| ()))
            .is_some()
    })
}

/// Resolve a follow-up for a message that named no agent.
fn resolve_followup(
    state: &AppState,
    message: &crate::control::ChatMessage,
    cfg: &AgentConfig,
) -> Option<super::engagement::Engagement> {
    if !super::engagement::is_followup_candidate(message, mentions_an_agent(message)) {
        return None;
    }
    let history: Vec<_> = state
        .transcript()
        .into_iter()
        .filter(|m| m.id != message.id)
        .collect();
    // An explicit reply-to wins outright: the user pointed at a message, which
    // is addressing, so no window and no recency guess applies (§1.4).
    if let Some(reply_to) = &message.reply_to {
        let target = super::engagement::resolve_reply_to(&history, reply_to);
        return target.filter(|e| e.peer_id == state.peer_id);
    }
    let engagement = super::engagement::resolve(
        &history,
        &message.peer_id,
        message.ts,
        cfg.engagement_window_secs,
        state.engagement_dismissed(&message.peer_id).as_ref(),
    )?;
    // A follow-up must wake the machine that ran the reply — and only that
    // machine. Every device evaluates this rule, so without the check they
    // would all answer.
    (engagement.peer_id == state.peer_id).then_some(engagement)
}

/// One agent turn waiting to run. Owned, because it outlives the event that
/// produced it.
struct QueuedTurn {
    agent_id: String,
    cmd: super::config::AgentCommand,
    task: String,
    sender: String,
    message_id: String,
    initiator: Initiator,
    relay: Option<Relay>,
    ambient: bool,
}

/// How many turns may wait for one agent before the oldest is dropped.
///
/// Deep enough to absorb someone typing three messages in a row while an agent
/// works; shallow enough that a queue cannot silently grow into a backlog the
/// user has forgotten about.
const MAX_QUEUED_PER_AGENT: usize = 4;

/// Serialized run queue for a Circle.
///
/// `driver::launch` refuses to start while any managed change session is open,
/// and that lock is Circle-wide. Before this, a mention arriving during a run
/// hit the refusal and was turned into a `system` chat post — a failure notice
/// for something the user is entitled to do. Follow-up routing (§1.1) makes
/// consecutive messages normal, which would have made that the common case.
///
/// So turns queue instead of racing. A single worker drains them in arrival
/// order, which also means the Circle-wide lock is never contended from here.
///
/// The spec (§1.3) would rather the lock were per-agent so unrelated agents run
/// in parallel. That needs `LocalChangeSession` to hold more than one open
/// managed session and the proposal baseline to tolerate two concurrent
/// writers, which it does not today — so this takes the fallback the spec
/// names: keep the Circle-wide lock and queue across it.
#[derive(Clone)]
struct RunQueue {
    pending: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<QueuedTurn>>>,
    wake: std::sync::Arc<tokio::sync::Notify>,
}

impl RunQueue {
    fn new() -> Self {
        Self {
            pending: Default::default(),
            wake: Default::default(),
        }
    }

    /// Enqueue a turn. Returns the message id of a turn dropped to make room,
    /// if the agent's queue was already full.
    fn push(&self, turn: QueuedTurn) -> Option<String> {
        let mut pending = self.pending.lock().unwrap();
        let agent = turn.agent_id.clone();
        let queued = pending.iter().filter(|t| t.agent_id == agent).count();
        // Drop the *oldest* rather than refusing the newest: the most recent
        // message is the one the user is waiting on, and an older one is more
        // likely to have been superseded by it.
        let dropped = if queued >= MAX_QUEUED_PER_AGENT {
            pending
                .iter()
                .position(|t| t.agent_id == agent)
                .and_then(|i| pending.remove(i))
                .map(|t| t.message_id)
        } else {
            None
        };
        pending.push_back(turn);
        drop(pending);
        self.wake.notify_one();
        dropped
    }

    fn pop(&self) -> Option<QueuedTurn> {
        self.pending.lock().unwrap().pop_front()
    }

    /// How many turns are waiting for this agent. The composer derives its
    /// "will be queued" hint from the agent's activity indicator instead, so
    /// this exists for tests.
    #[cfg(test)]
    fn depth_for(&self, agent: &str) -> usize {
        self.pending
            .lock()
            .unwrap()
            .iter()
            .filter(|t| t.agent_id == agent)
            .count()
    }
}

/// Drain the queue one turn at a time for the life of the Circle.
fn spawn_run_worker(state: AppState, queue: RunQueue, token: CancellationToken) {
    tokio::spawn(async move {
        loop {
            let Some(turn) = queue.pop() else {
                tokio::select! {
                    _ = token.cancelled() => break,
                    _ = queue.wake.notified() => continue,
                }
            };
            if token.is_cancelled() {
                break;
            }
            run_one(&state, turn).await;
        }
    });
}

async fn run_one(state: &AppState, turn: QueuedTurn) {
    let QueuedTurn {
        agent_id,
        cmd,
        task,
        sender,
        message_id,
        initiator,
        relay,
        ambient,
    } = turn;
    if let Err(e) = react(
        state,
        Turn {
            agent_id: &agent_id,
            cmd: &cmd,
            task: &task,
            sender: &sender,
            message_id: &message_id,
            initiator,
            relay,
            ambient,
        },
    )
    .await
    {
        publish_agent_activity(
            state,
            &agent_id,
            &message_id,
            ChatActivityKind::Working,
            false,
        );
        tracing::warn!("[agent] run of `{agent_id}` failed: {e:#}");
        let reason = concise_error(&e);
        let text = format!("@{agent_id} failed to start · {reason}");
        let _ = crate::api::chat::post_message(
            state,
            "system".to_string(),
            text,
            crate::api::chat::Trigger::System,
        );
    }
}

/// This device's own count of agent turns it has run per cascade root.
///
/// The `spent` field on the wire is a hint from a peer that could have forged
/// it, so it can only ever *shrink* a budget, never extend one. This ledger is
/// the local truth: a cascade that re-enters this device several times cannot
/// spend more turns here than this device allows in total, whatever the wire
/// says.
///
/// Bounded by construction — an entry is only made for a root that actually
/// ran something here, and the map is dropped with the daemon. A restart
/// forgetting a cascade is acceptable: the cascade is long over.
#[derive(Default)]
struct RelayLedger {
    spent: std::sync::Mutex<std::collections::HashMap<String, u8>>,
}

impl RelayLedger {
    /// Charge one turn against `root` if this device's ceiling allows it.
    /// Returns false when the cascade has already cost this device its limit.
    fn charge(&self, root: &str, max: u8) -> bool {
        let ceiling = max.min(crate::agent::relay::RELAY_TURNS_CEILING);
        let mut spent = self.spent.lock().unwrap();
        let entry = spent.entry(root.to_string()).or_insert(0);
        if *entry >= ceiling {
            return false;
        }
        *entry += 1;
        true
    }
}

fn concise_error(error: &anyhow::Error) -> String {
    let full = format!("{error:#}").replace(['\r', '\n'], " ");
    let mut chars = full.chars();
    let short: String = chars.by_ref().take(240).collect();
    if chars.next().is_some() {
        format!("{short}…")
    } else {
        short
    }
}

/// One agent turn: everything the run needs that is not the daemon state.
struct Turn<'a> {
    agent_id: &'a str,
    cmd: &'a super::config::AgentCommand,
    task: &'a str,
    sender: &'a str,
    message_id: &'a str,
    initiator: Initiator,
    /// The cascade that woke this turn, carried forward onto its reply so a
    /// mention in that reply spends from the same budget.
    relay: Option<Relay>,
    /// An unaddressed turn, which may decline with PASS.
    ambient: bool,
}

async fn react(state: &AppState, turn: Turn<'_>) -> anyhow::Result<()> {
    let Turn {
        agent_id,
        cmd,
        task,
        sender,
        message_id,
        initiator,
        relay,
        ambient,
    } = turn;
    publish_agent_activity(state, agent_id, message_id, ChatActivityKind::Working, true);

    // Anchor the change session on the engine's current baseline (S0) so the
    // agent's edits diff cleanly against it.
    let store = ProposalStore::open(&state.workspace)?;
    let base_snapshot = store.baseline_id().unwrap_or_default();

    // Resume the agent's prior conversation if we remember one. Best-effort:
    // the driver falls back to a fresh session if the id no longer loads.
    let resume = super::memory::load(&state.circle_dir, agent_id);

    // Give the agent enough context about where it is. On a resumed session the
    // agent already has history, so we send a lean per-turn header; on a fresh
    // session we include the standing brief about the enoxian environment.
    let mut prompt =
        super::context::build_prompt(state, agent_id, sender, task, resume.as_ref(), message_id);
    if ambient {
        prompt.push_str("\n\n");
        prompt.push_str(super::ambient::ambient_instruction());
    }
    let (actor_token, _) = state
        .actor_tokens
        .issue(&state.circle_id, &state.peer_id, agent_id);

    let launch = driver::launch(driver::LaunchRequest {
        agent_name: agent_id,
        cmd,
        task: &prompt,
        workspace: &state.workspace,
        base_snapshot: &base_snapshot,
        circle_id: &state.circle_id,
        circle_dir: &state.circle_dir,
        actor_token: Some(&actor_token),
        relay_path: relay.as_ref().map(|r| r.path.clone()).unwrap_or_default(),
        initiator,
        resume: resume.as_ref().map(|r| r.session_id.as_str()),
    });
    tokio::pin!(launch);

    // Long agent runs renew their lease. If this process disappears, peers
    // naturally hide the indicator after the last 45-second lease expires.
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(15));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    heartbeat.tick().await;
    let outcome = loop {
        tokio::select! {
            result = &mut launch => break result?,
            _ = heartbeat.tick() => publish_agent_activity(
                state,
                agent_id,
                message_id,
                ChatActivityKind::Working,
                true,
            ),
        }
    };

    // Remember the ACP session so the next mention continues the conversation.
    if let Some(sid) = &outcome.acp_session_id {
        if let Err(e) = super::memory::save_session(&state.circle_dir, agent_id, sid) {
            tracing::warn!("[agent] failed to persist session for `{agent_id}`: {e}");
        }
    }

    tracing::info!(
        "[agent] `{agent_id}` finished: session={} {}",
        outcome.session_id,
        outcome.detail
    );

    // Post the agent's streamed reply back into the chat room so the mention
    // reads like a conversation. File changes still surface separately as a
    // proposal (via the ambient engine + pull protocol); this is the
    // conversational half.
    // An unaddressed agent that declines says PASS (§2.3). Suppress the post,
    // but still advance its seen-mark and say so in the activity indicator —
    // "considered and passed" must not look like "never ran".
    if ambient
        && outcome
            .reply
            .as_deref()
            .is_some_and(super::ambient::is_pass)
    {
        tracing::info!("[agent] `{agent_id}` passed on {message_id}");
        mark_seen(state, agent_id, message_id);
        publish_agent_activity_detailed(
            state,
            agent_id,
            message_id,
            ChatActivityKind::Skipped,
            true,
            Some("nothing to add".to_string()),
        );
        return Ok(());
    }
    if let Some(reply) = outcome
        .reply
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        // Post under the agent's name, carrying this cascade's relay forward.
        // A mention in the reply may wake another agent, but only within the
        // budget the rooting human message minted — see `agent::relay`.
        let posted = crate::api::chat::post_message(
            state,
            agent_id.to_string(),
            reply.to_string(),
            crate::api::chat::Trigger::AgentReply {
                agent: agent_id.to_string(),
                parent: relay,
            },
        );
        mark_seen(state, agent_id, posted.as_deref().unwrap_or(message_id));
    } else {
        tracing::debug!("[agent] `{agent_id}` produced no text reply to post");
        mark_seen(state, agent_id, message_id);
    }
    publish_agent_activity(
        state,
        agent_id,
        message_id,
        ChatActivityKind::Working,
        false,
    );
    Ok(())
}

/// Remember the last chat line this agent has seen — its own reply, or the
/// mention it just handled when it said nothing — so the next turn carries only
/// what the room said in between. Best-effort: a failure here costs the next
/// prompt a few already-seen lines, not correctness.
fn mark_seen(state: &AppState, agent_id: &str, message_id: &str) {
    if let Err(e) = super::memory::save_seen(&state.circle_dir, agent_id, message_id) {
        tracing::debug!("[agent] failed to persist seen-mark for `{agent_id}`: {e}");
    }
}

fn publish_agent_activity(
    state: &AppState,
    agent_id: &str,
    message_id: &str,
    kind: ChatActivityKind,
    live: bool,
) {
    publish_agent_activity_detailed(state, agent_id, message_id, kind, live, None)
}

fn publish_agent_activity_detailed(
    state: &AppState,
    agent_id: &str,
    message_id: &str,
    kind: ChatActivityKind,
    live: bool,
    detail: Option<String>,
) {
    let now = chrono::Utc::now().timestamp();
    let activity = ChatActivity {
        activity_id: format!("agent:{message_id}:{agent_id}:{}", state.peer_id),
        actor_id: agent_id.to_string(),
        peer_id: state.peer_id.clone(),
        kind,
        detail,
        message_id: Some(message_id.to_string()),
        updated_at: now,
        expires_at: if live {
            now + crate::api::chat::AGENT_ACTIVITY_TTL_SECS
        } else {
            now - 1
        },
    };
    if let Err(error) = crate::api::chat::put_activity(state, activity) {
        tracing::debug!("[agent] failed to publish chat activity: {error}");
    }
}

/// Whether a mention scoped to `owner/device` addresses this device. Matches
/// this circle's owner and the local device label (case-insensitive, since
/// users type these by hand). If the local device has no label set, a device-
/// scoped mention can never match it — the user should set one to be
/// addressable (`enox identity set-label`).
/// Does `@owner/device/...` address *this* machine?
///
/// The comparison must be made against the identity the **Circle** knows this
/// device by — its roster entry — not against the local `identity.toml`.
/// A mention is composed from the roster (that is what the `@` autocomplete
/// offers), so comparing it to a different source lets the two drift: rename a
/// device, restore an `~/.enoxian` from another machine, or join with a label
/// that was later changed, and the local file no longer says what the rest of
/// the Circle calls this machine. When it drifts, targeting fails in both
/// directions at once — the addressed device ignores the mention while another
/// device answers for it.
///
/// The roster entry is matched by `peer_id`, the only field that identifies a
/// machine. `identity.toml` remains the fallback for the window before this
/// device's own entry has synced.
pub(crate) fn targets_this_device(state: &AppState, owner: &str, device: &str) -> bool {
    let (local_owner, local_device) = match state.self_member() {
        Some(me) if !me.device_label.is_empty() => (me.owner, me.device_label),
        // Not in the roster yet: fall back to the local identity file.
        _ => (
            state.owner.clone(),
            crate::identity::read_identity_display()
                .map(|(label, _)| label)
                .unwrap_or_default(),
        ),
    };
    if local_device.is_empty() {
        // No idea what this device is called. Refusing is the safe answer: a
        // device that cannot confirm it is the target must not answer for one
        // that is.
        tracing::warn!(
            "[agent] mention scoped to {owner}/{device} ignored — this device has no known label"
        );
        return false;
    }
    let matches =
        local_owner.eq_ignore_ascii_case(owner) && local_device.eq_ignore_ascii_case(device);
    tracing::debug!(
        "[agent] mention scoped to {owner}/{device}; this device is {local_owner}/{local_device} — {}",
        if matches { "running it" } else { "not ours" }
    );
    matches
}

/// Strip a leading `@mention` (and following whitespace) from the message so the
/// agent receives just the task. A mention elsewhere in the text is left alone.
/// The mention body may be scoped (`@owner/device/agent`).
fn strip_mention(text: &str, mention_body: &str) -> String {
    let trimmed = text.trim_start();
    let needle = format!("@{mention_body}");
    if let Some(rest) = trimmed.strip_prefix(&needle) {
        rest.trim().to_string()
    } else {
        trimmed.trim_end().to_string()
    }
}

/// Is this run attributable to the local user?
///
/// For a direct mention that is just "did this device post it". For a relayed
/// turn the answer must come from the human at the root of the cascade: the
/// agent that mentioned us may well be running on this device while the person
/// who actually asked is a remote member.
fn attributed_local(state: &AppState, message: &crate::control::ChatMessage) -> bool {
    match &message.relay {
        // A cascade rooted in a human message this device posted.
        Some(relay) if !relay.root_peer.is_empty() => relay.root_peer == state.peer_id,
        _ => message.agent_id == state.agent_id,
    }
}

/// Surface a delegation that was considered and not run.
///
/// Deliberately *not* a `system` chat post: a failure notice per dead mention
/// is exactly the transcript noise a busy cascade would fill the room with. It
/// shows in the activity indicator instead, where it is legible while it
/// matters and gone afterwards.
fn publish_relay_skipped(state: &AppState, agent: &str, message_id: &str, reason: &str) {
    publish_agent_activity_detailed(
        state,
        agent,
        message_id,
        ChatActivityKind::Skipped,
        true,
        Some(reason.to_string()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_leading_mention() {
        assert_eq!(
            strip_mention("@claude fix the docs", "claude"),
            "fix the docs"
        );
        assert_eq!(strip_mention("  @claude  do it ", "claude"), "do it");
    }

    #[test]
    fn leaves_non_leading_mention() {
        assert_eq!(
            strip_mention("please ping @claude later", "claude"),
            "please ping @claude later"
        );
    }

    #[test]
    fn failure_message_is_single_line_and_bounded() {
        let error = anyhow::anyhow!("first line\n{}", "x".repeat(300));
        let text = concise_error(&error);
        assert!(!text.contains('\n'));
        assert!(text.chars().count() <= 241);
    }

    use crate::control::{MemberEntry, MemberRole, MEMBER_LIST_KEY};
    use yrs::{Any, Map, Transact, WriteTxn};

    fn test_state(peer: &str, owner: &str) -> (AppState, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(
            "c1".into(),
            "circle".into(),
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            String::new(),
            "label".into(),
            1,
            peer.into(),
            crate::config::JoinPolicy::Manual,
            owner.into(),
            crate::mls::new_mls_state(crate::mls::MlsIdentity::generate(peer).unwrap(), None),
        );
        (state, dir)
    }

    fn add_member(state: &AppState, peer: &str, owner: &str, device: &str) {
        let entry = MemberEntry {
            peer_id: peer.into(),
            owner: owner.into(),
            agent_id: format!("{owner}-{device}"),
            device_label: device.into(),
            agents: vec!["suzent".into()],
            ambient_agents: Vec::new(),
            role: MemberRole::Admin,
            added_at: chrono::Utc::now(),
            signature: String::new(),
        };
        let mut txn = state.control.try_transact_mut().unwrap();
        let map = txn.get_or_insert_map(MEMBER_LIST_KEY);
        let json = serde_json::to_string(&entry).unwrap();
        map.insert(&mut txn, peer, Any::String(json.as_str().into()));
    }

    #[test]
    fn a_device_runs_only_mentions_addressed_to_it() {
        let (state, _d) = test_state("peer-jessair", "suzy");
        add_member(&state, "peer-jessair", "suzy", "jessair");
        add_member(&state, "peer-macbook", "suzy", "macbook-pro");

        assert!(targets_this_device(&state, "suzy", "jessair"));
        assert!(
            !targets_this_device(&state, "suzy", "macbook-pro"),
            "jessair must not answer a mention addressed to macbook-pro"
        );
    }

    #[test]
    fn the_roster_label_wins_over_a_drifted_local_identity() {
        // The real failure: a device whose local identity.toml disagrees with
        // the label the Circle addresses it by. Mentions are composed from the
        // roster, so the roster is what targeting must compare against —
        // otherwise the addressed device ignores the mention while another
        // device answers for it.
        let (state, _d) = test_state("peer-jessair", "suzy");
        add_member(&state, "peer-jessair", "suzy", "jessair");
        assert!(targets_this_device(&state, "suzy", "jessair"));
        assert!(!targets_this_device(&state, "suzy", "macbook-pro"));
    }

    #[test]
    fn a_different_owner_never_matches() {
        let (state, _d) = test_state("peer-jessair", "suzy");
        add_member(&state, "peer-jessair", "suzy", "jessair");
        assert!(!targets_this_device(&state, "alice", "jessair"));
    }

    #[test]
    fn matching_is_case_insensitive() {
        let (state, _d) = test_state("peer-jessair", "suzy");
        add_member(&state, "peer-jessair", "suzy", "jessair");
        assert!(targets_this_device(&state, "SUZY", "JessAir"));
    }

    #[test]
    fn a_device_with_no_known_label_refuses_rather_than_guesses() {
        // Not in the roster and no usable local label: answering for a device
        // it cannot prove it is would be worse than staying quiet.
        let (state, _d) = test_state("peer-unknown", "suzy");
        let unknown = targets_this_device(&state, "suzy", "macbook-pro");
        // Falls back to identity.toml; on a machine with no label it must be
        // false. Where a label exists it must at least not match a device it
        // is not.
        if unknown {
            let local = crate::identity::read_identity_display()
                .map(|(l, _)| l)
                .unwrap_or_default();
            assert!(
                local.eq_ignore_ascii_case("macbook-pro"),
                "matched a device it is not: local label is {local:?}"
            );
        }
    }

    fn queued(agent: &str, message_id: &str) -> QueuedTurn {
        QueuedTurn {
            agent_id: agent.into(),
            cmd: super::super::config::AgentCommand::default(),
            task: String::new(),
            sender: "suzy".into(),
            message_id: message_id.into(),
            initiator: Initiator::Local,
            relay: None,
            ambient: false,
        }
    }

    #[test]
    fn turns_run_in_arrival_order() {
        let q = RunQueue::new();
        for id in ["m1", "m2", "m3"] {
            assert!(q.push(queued("claude", id)).is_none());
        }
        let order: Vec<String> = std::iter::from_fn(|| q.pop())
            .map(|t| t.message_id)
            .collect();
        assert_eq!(order, ["m1", "m2", "m3"]);
    }

    #[test]
    fn a_busy_agent_queues_instead_of_failing() {
        // The behaviour that replaces "@agent failed to start · already
        // running": a second message during a run is accepted, not refused.
        let q = RunQueue::new();
        assert!(q.push(queued("claude", "m1")).is_none());
        assert!(q.push(queued("claude", "m2")).is_none());
        assert_eq!(q.depth_for("claude"), 2);
    }

    #[test]
    fn one_agents_backlog_does_not_squeeze_out_another() {
        let q = RunQueue::new();
        for i in 0..MAX_QUEUED_PER_AGENT {
            assert!(q.push(queued("claude", &format!("c{i}"))).is_none());
        }
        assert!(
            q.push(queued("codex", "x1")).is_none(),
            "the cap is per agent, not per Circle"
        );
        assert_eq!(q.depth_for("codex"), 1);
    }

    #[test]
    fn an_overfull_queue_drops_the_oldest_and_says_which() {
        let q = RunQueue::new();
        for i in 0..MAX_QUEUED_PER_AGENT {
            assert!(q.push(queued("claude", &format!("m{i}"))).is_none());
        }
        // The newest message is the one the user is waiting on, so the oldest
        // goes — and the caller is told, because this loses a real message.
        assert_eq!(q.push(queued("claude", "new")).as_deref(), Some("m0"));
        assert_eq!(q.depth_for("claude"), MAX_QUEUED_PER_AGENT);
        let remaining: Vec<String> = std::iter::from_fn(|| q.pop())
            .map(|t| t.message_id)
            .collect();
        assert_eq!(remaining, ["m1", "m2", "m3", "new"]);
    }

    #[test]
    fn an_empty_queue_pops_nothing() {
        assert!(RunQueue::new().pop().is_none());
    }

    #[test]
    fn ledger_stops_a_cascade_at_this_devices_ceiling() {
        let ledger = RelayLedger::default();
        for _ in 0..3 {
            assert!(ledger.charge("root-a", 3));
        }
        assert!(!ledger.charge("root-a", 3));
        // A different cascade is unaffected — the budget is per root, not
        // per device-lifetime.
        assert!(ledger.charge("root-b", 3));
    }

    #[test]
    fn ledger_honours_the_hard_ceiling_over_config() {
        let ledger = RelayLedger::default();
        for _ in 0..crate::agent::relay::RELAY_TURNS_CEILING {
            assert!(ledger.charge("root", 250));
        }
        assert!(!ledger.charge("root", 250));
    }
}
