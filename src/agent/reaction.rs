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

use super::config::{AgentConfig, Reaction};
use super::driver::{self, Initiator};
use super::mention::Mention;
use crate::control::{ChatActivity, ChatActivityKind, CircleEvent, Relay};
use crate::proposal::store::ProposalStore;
use crate::state::AppState;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

pub fn spawn_reaction(state: AppState, token: CancellationToken) {
    tokio::spawn(async move {
        loop {
            match run(state.clone(), token.clone()).await {
                Ok(()) => break,
                Err(error) => tracing::warn!("[agent] reaction loop unavailable: {error:#}"),
            }
            // A previous Circle instance may still be finishing a process and
            // holding inbox ownership after disable/re-enable. Retry acquisition
            // without resetting history or starting a second owner.
            tokio::select! {
                _ = token.cancelled() => break,
                _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {},
            }
        }
    });
}

async fn run(state: AppState, token: CancellationToken) -> anyhow::Result<()> {
    let mut events = state.events.subscribe();
    let handled = super::handled::HandledMentions::load(&state.circle_dir);
    let inbox = std::sync::Arc::new(super::inbox::Inbox::open(
        &state.circle_dir,
        chrono::Utc::now().timestamp(),
    )?);
    *state.execution_inbox.write().unwrap() = Some(std::sync::Arc::downgrade(&inbox));
    let wake = std::sync::Arc::new(tokio::sync::Notify::new());
    let worker_token = token.child_token();
    let _cancel_worker = worker_token.clone().drop_guard();
    let mut worker = spawn_run_worker(state.clone(), inbox.clone(), wake.clone(), worker_token);
    // This boundary is only for ambient observations. Addressed work uses the
    // persisted activation boundary, not a new cutoff on every daemon start.
    let live_since = chrono::Utc::now().timestamp() - 2;
    let mut reconcile = tokio::time::interval(std::time::Duration::from_secs(5));
    reconcile.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = token.cancelled() => break,
            result = &mut worker => {
                result??;
                anyhow::bail!("execution worker stopped unexpectedly; recovering owner");
            },
            _ = reconcile.tick() => {
                if let Err(error) = crate::proposal::runs::release_finished_locks(&state) { tracing::debug!("lock cleanup deferred: {error}"); }
                reconcile_requests(&state, &handled, &inbox, &wake);
            }
            evt = events.recv() => match evt {
                Ok(CircleEvent::MessagePosted { message }) => {
                    let cfg = AgentConfig::load();
                    admit_message(&state, &handled, &inbox, &wake, &cfg, &message,
                        message.ts >= live_since);
                }
                Ok(CircleEvent::RelayStopped { root }) => {
                    for entry in inbox.entries().into_iter().filter(|e| e.status == super::inbox::Status::Pending && e.request.relay.as_ref().is_some_and(|r| r.root == root)) {
                        inbox.transition(&entry.run_id, super::inbox::Status::Pending, super::inbox::Status::Cancelled,
                            Some("chain stopped".into()), chrono::Utc::now().timestamp())?;
                    }
                    wake.notify_one();
                }
                // MessagePosted is the single admission path. AgentMentioned
                // is also emitted for the same message and must not fan out a
                // second time, especially when a peer replays the transcript.
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("[agent] reaction stream lagged by {n}; reconciling inbox");
                    reconcile_requests(&state, &handled, &inbox, &wake);
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
        anyhow::ensure!(
            inbox.is_healthy(),
            "execution inbox unavailable; recovering owner"
        );
    }
    Ok(())
}

fn reconcile_requests(
    state: &AppState,
    handled: &super::handled::HandledMentions,
    inbox: &super::inbox::Inbox,
    wake: &tokio::sync::Notify,
) {
    reconcile_requests_with_config(state, handled, inbox, wake, &AgentConfig::load());
}

fn reconcile_requests_with_config(
    state: &AppState,
    handled: &super::handled::HandledMentions,
    inbox: &super::inbox::Inbox,
    wake: &tokio::sync::Notify,
    cfg: &AgentConfig,
) {
    let mut history = state.transcript();
    for (message_id, mention_key) in handled.entries() {
        let Some(message) = history.iter().find(|m| m.id == message_id) else {
            continue;
        };
        let Some(mention) = Mention::parse(&mention_key) else {
            continue;
        };
        let Some((agent, scope)) = mention.agent_target() else {
            continue;
        };
        if scope.is_some_and(|(owner, device)| !targets_this_device(state, owner, device)) {
            continue;
        }
        let request = super::inbox::Request {
            agent: agent.to_string(),
            mention_key: mention_key.clone(),
            message: message.clone(),
            task: strip_mention(&message.text, &mention_key),
            relay: message.relay.clone(),
            implicit: false,
            ambient: false,
        };
        if let Err(error) = inbox.record_suppressed(request, message.ts) {
            tracing::warn!("legacy inbox import failed: {error}");
        }
    }
    history.sort_by(|a, b| a.ts.cmp(&b.ts).then(a.id.cmp(&b.id)));
    for message in history {
        // Reconciliation delivers explicit work, never stale room observations
        // or recency guesses newly invented by later transcript changes.
        if !message.mentions.is_empty() || message.reply_to.is_some() {
            admit_message(state, handled, inbox, wake, cfg, &message, false);
        }
    }
    let _ = crate::api::execution::publish(state, inbox);
    wake.notify_one();
}

fn message_author_scope(state: &AppState, peer: &str) -> Option<(String, String)> {
    use yrs::{Any, Map, Out, ReadTxn, Transact};
    let txn = state.control.try_transact().ok()?;
    let members = txn.get_map(crate::control::MEMBER_LIST_KEY)?;
    members.iter(&txn).find_map(|(_, value)| {
        let Out::Any(Any::String(raw)) = value else {
            return None;
        };
        let member: crate::control::MemberEntry = serde_json::from_str(&raw).ok()?;
        (member.peer_id == peer).then_some((member.owner, member.device_label))
    })
}

fn admit_message(
    state: &AppState,
    handled: &super::handled::HandledMentions,
    inbox: &super::inbox::Inbox,
    wake: &tokio::sync::Notify,
    cfg: &AgentConfig,
    message: &crate::control::ChatMessage,
    live: bool,
) {
    if message.ts < inbox.activated_at() || message.author == crate::control::Author::System {
        return;
    }
    let agent_reply = message.author == crate::control::Author::Agent
        || message
            .relay
            .as_ref()
            .and_then(super::relay::poster)
            .is_some();
    let mentions = if agent_reply {
        // The author's scope is taken from its peer, not this receiving device.
        // Never allow a missing relay on an agent-authored message to mint work.
        if message
            .relay
            .as_ref()
            .and_then(super::relay::poster)
            .is_none()
        {
            return;
        }
        let writer = message_author_scope(state, &message.peer_id);
        super::relay::triggerable_mentions(
            message,
            super::relay::RELAY_TURNS_CEILING,
            writer
                .as_ref()
                .map(|(owner, device)| (owner.as_str(), device.as_str())),
        )
    } else {
        message.mentions.clone()
    };
    for body in mentions {
        let Some(mention) = Mention::parse(&body) else {
            continue;
        };
        let Some((agent, scope)) = mention.agent_target() else {
            continue;
        };
        if scope.is_some_and(|(owner, device)| !targets_this_device(state, owner, device)) {
            continue;
        }
        dispatch(
            state,
            handled,
            inbox,
            wake,
            cfg,
            DispatchRequest {
                agent,
                mention_key: &body,
                task: strip_mention(&message.text, &body),
                message,
                relay: message.relay.clone(),
                implicit: false,
                ambient: false,
            },
        );
    }
    let engagement = if live || message.reply_to.is_some() {
        resolve_followup(state, message, cfg)
    } else {
        None
    };
    if live
        && message.ts >= chrono::Utc::now().timestamp() - 30
        && ambient_route_allowed(message, engagement.as_ref())
    {
        offer_ambient(state, handled, inbox, wake, cfg, message);
    }
    if let Some(engagement) = engagement.filter(|e| e.peer_id == state.peer_id) {
        dispatch(
            state,
            handled,
            inbox,
            wake,
            cfg,
            DispatchRequest {
                agent: &engagement.agent,
                mention_key: &super::engagement::dedup_key(&engagement.agent),
                task: message.text.clone(),
                message,
                relay: Some(super::relay::mint(&message.id, &message.peer_id)),
                implicit: true,
                ambient: false,
            },
        );
    }
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
    inbox: &super::inbox::Inbox,
    wake: &tokio::sync::Notify,
    cfg: &AgentConfig,
    req: DispatchRequest<'_>,
) {
    let settings = cfg.resolved(&state.circle_id);
    if settings.reaction != Reaction::Push || cfg.resolve(req.agent).is_none() {
        return;
    }
    // Imported markers suppress replay but are never called completed jobs.
    if handled.contains(&req.message.id, req.mention_key) {
        return;
    }
    let request = super::inbox::Request {
        agent: req.agent.to_string(),
        mention_key: req.mention_key.to_string(),
        message: req.message.clone(),
        task: req.task,
        relay: req.relay,
        implicit: req.implicit,
        ambient: req.ambient,
    };
    if request.delegated()
        && request.message.peer_id == state.peer_id
        && request.relay.as_ref().and_then(super::relay::poster) == Some(req.agent)
    {
        return;
    }
    match inbox.admit(
        request,
        settings.max_relay_turns,
        chrono::Utc::now().timestamp(),
    ) {
        Ok(super::inbox::Admission::Duplicate) => {}
        Ok(super::inbox::Admission::Rejected(entry)) => {
            publish_relay_skipped(
                state,
                &entry.request.agent,
                &entry.request.message.id,
                entry.detail.as_deref().unwrap_or("not admitted"),
            );
        }
        Ok(super::inbox::Admission::Accepted { entry, displaced }) => {
            publish_agent_activity(
                state,
                &entry.request.agent,
                &entry.request.message.id,
                ChatActivityKind::Seen,
                true,
            );
            for dropped in displaced {
                publish_relay_skipped(
                    state,
                    &dropped.request.agent,
                    &dropped.request.message.id,
                    dropped
                        .detail
                        .as_deref()
                        .unwrap_or("queue capacity exceeded"),
                );
            }
            wake.notify_one();
        }
        Err(error) => {
            tracing::error!("[agent] could not persist request: {error:#}");
            publish_relay_skipped(
                state,
                req.agent,
                &req.message.id,
                "execution inbox could not be saved",
            );
        }
    }
}

/// Offer an unaddressed message to this device's ambient agents (§2).
fn offer_ambient(
    state: &AppState,
    handled: &super::handled::HandledMentions,
    inbox: &super::inbox::Inbox,
    wake: &tokio::sync::Notify,
    cfg: &AgentConfig,
    message: &crate::control::ChatMessage,
) {
    // Which agents read the room *here*. An agent can be ambient in a working
    // Circle and silent in a social one, so the answer is per Circle.
    let settings = cfg.resolved(&state.circle_id);
    let ambient: Vec<String> = settings
        .ambient
        .iter()
        .filter(|name| cfg.agents.contains_key(*name))
        .cloned()
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
            inbox,
            wake,
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

/// Decide ambient eligibility before device filtering: a remote target is
/// still an addressed conversation. An unresolved explicit reply must not
/// silently become an invitation to unrelated ambient agents.
fn ambient_route_allowed(
    message: &crate::control::ChatMessage,
    engagement: Option<&super::engagement::Engagement>,
) -> bool {
    message.reply_to.is_none() && engagement.is_none() && !mentions_an_agent(message)
}

/// Resolve a follow-up for a message that named no agent, including remote
/// targets. The caller filters execution to the target device after deciding
/// whether the message belongs to an addressed conversation.
fn resolve_followup(
    state: &AppState,
    message: &crate::control::ChatMessage,
    cfg: &AgentConfig,
) -> Option<super::engagement::Engagement> {
    if !super::engagement::is_followup_candidate(message, mentions_an_agent(message)) {
        return None;
    }
    let mut history = state.transcript();
    // An explicit reply-to wins outright: the user pointed at a message, which
    // is addressing, so no window and no recency guess applies (§1.4).
    if let Some(reply_to) = &message.reply_to {
        return super::engagement::resolve_reply_to(&history, reply_to);
    }
    // Only history preceding this message may establish a recency target.
    // Keep same-second ordering from the transcript rather than dropping valid
    // rapid replies or consulting later agent answers during reconciliation.
    if let Some(index) = history.iter().position(|m| m.id == message.id) {
        history.truncate(index);
    }
    history.retain(|m| m.ts <= message.ts);
    super::engagement::resolve(
        &history,
        &message.peer_id,
        message.ts,
        cfg.resolved(&state.circle_id).engagement_window_secs,
        state.engagement_dismissed(&message.peer_id).as_ref(),
    )
}

/// A FIFO device permit bounds provider processes across all Circles. Each
/// Circle offers at most one candidate per agent; the conversation lease also
/// covers manual CLI launches. Configuration changes resize on daemon restart.
fn device_permits() -> &'static std::sync::Arc<tokio::sync::Semaphore> {
    static POOL: std::sync::OnceLock<std::sync::Arc<tokio::sync::Semaphore>> =
        std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        std::sync::Arc::new(tokio::sync::Semaphore::new(
            AgentConfig::load().max_concurrent_runs.clamp(1, 32),
        ))
    })
}

fn spawn_run_worker(
    state: AppState,
    inbox: std::sync::Arc<super::inbox::Inbox>,
    wake: std::sync::Arc<tokio::sync::Notify>,
    token: CancellationToken,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    tokio::spawn(async move {
        let mut failure = None;
        let mut active = std::collections::HashSet::new();
        let mut tasks = tokio::task::JoinSet::new();
        let mut paused = std::collections::HashSet::new();
        loop {
            if token.is_cancelled() {
                break;
            }
            for entry in inbox
                .entries()
                .into_iter()
                .filter(|e| e.status == super::inbox::Status::Pending)
            {
                let agent = entry.request.agent;
                if paused.contains(&agent) || !active.insert(agent.clone()) {
                    continue;
                }
                let state = state.clone();
                let inbox = inbox.clone();
                let cancel = token.clone();
                tasks.spawn(async move {
                    let result = async {
                        let _permit = tokio::select! {
                            _ = cancel.cancelled() => return Ok(false),
                            permit = device_permits().clone().acquire_owned() => permit?,
                        };
                        // Policy and chain stops are checked after acquiring the
                        // permit, immediately before the durable running claim.
                        run_next_cancellable(
                            &state,
                            &inbox,
                            &AgentConfig::load(),
                            Some(&agent),
                            &cancel,
                        )
                        .await
                    }
                    .await;
                    (agent, result)
                });
            }
            tokio::select! {
                _ = token.cancelled() => break,
                _ = wake.notified() => { paused.clear(); },
                joined = tasks.join_next(), if !tasks.is_empty() => {
                    match joined {
                        Some(Ok((agent, Ok(progress)))) => {
                            active.remove(&agent);
                            if !progress { paused.insert(agent); }
                        },
                        Some(Ok((agent, Err(error)))) => {
                            tracing::error!("[agent] execution worker {agent} stopped: {error:#}");
                            failure = Some(error);
                            token.cancel();
                            break;
                        },
                        Some(Err(error)) => { failure = Some(error.into()); token.cancel(); break; },
                        None => {},
                    }
                }
            }
        }
        // Disable/stop-chain never implicitly kills an already running turn.
        while tasks.join_next().await.is_some() {}
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    })
}

#[cfg(test)]
async fn run_next_with_config(
    state: &AppState,
    inbox: &super::inbox::Inbox,
    cfg: &AgentConfig,
) -> anyhow::Result<bool> {
    run_next_for_agent(state, inbox, cfg, None).await
}

#[cfg(test)]
async fn run_next_for_agent(
    state: &AppState,
    inbox: &super::inbox::Inbox,
    cfg: &AgentConfig,
    agent: Option<&str>,
) -> anyhow::Result<bool> {
    run_next_cancellable(state, inbox, cfg, agent, &CancellationToken::new()).await
}

async fn run_next_cancellable(
    state: &AppState,
    inbox: &super::inbox::Inbox,
    cfg: &AgentConfig,
    agent: Option<&str>,
    cancel: &CancellationToken,
) -> anyhow::Result<bool> {
    use super::inbox::Status;
    if ProposalStore::open(&state.workspace)?
        .baseline_id()
        .is_none()
    {
        return Ok(false);
    }
    for entry in inbox
        .entries()
        .into_iter()
        .filter(|e| e.status == Status::Pending && agent.is_none_or(|a| e.request.agent == a))
    {
        let request = &entry.request;
        let rejection = match launch_rejection(state, request, cfg) {
            Ok(reason) => reason,
            Err(_) => continue, // Unknown stop state is retried, never treated as permission.
        };
        if let Some(reason) = rejection {
            inbox.transition(
                &entry.run_id,
                Status::Pending,
                Status::Cancelled,
                Some(reason.into()),
                chrono::Utc::now().timestamp(),
            )?;
            publish_relay_skipped(state, &request.agent, &request.message.id, reason);
            continue;
        }
        if cfg.resolved(&state.circle_id).reaction != Reaction::Push {
            continue;
        }
        let Some(cmd) = cfg.resolve(&request.agent) else {
            continue;
        };
        if !inbox.transition(
            &entry.run_id,
            Status::Pending,
            Status::Running,
            None,
            chrono::Utc::now().timestamp(),
        )? {
            continue;
        }
        let _ = crate::api::execution::publish(state, inbox);
        let initiator = if request.ambient {
            Initiator::Ambient
        } else if attributed_local(state, &request.message) {
            Initiator::Local
        } else {
            Initiator::RemoteMember
        };
        let result = react(
            state,
            Turn {
                run_id: &entry.run_id,
                agent_id: &request.agent,
                cmd,
                task: &request.task,
                sender: &request.message.agent_id,
                message_id: &request.message.id,
                initiator,
                relay: request.relay.clone(),
                ambient: request.ambient,
            },
            cancel,
        )
        .await;
        let (status, detail) = match result {
            Ok(()) => (Status::Completed, None),
            Err(error) if error.is::<crate::proposal::runs::ConversationBusy>() => (
                Status::Pending,
                Some("waiting for the previous conversation turn".into()),
            ),
            Err(error) if error.is::<crate::proposal::runs::DeviceCapacityUnavailable>() => (
                Status::Pending,
                Some("waiting for device capacity; no process launched".into()),
            ),
            Err(error) => {
                let reason = concise_error(&error);
                tracing::warn!("[agent] run {} failed: {error:#}", entry.run_id);
                publish_relay_skipped(state, &request.agent, &request.message.id, &reason);
                (Status::Failed, Some(reason))
            }
        };
        inbox.transition(
            &entry.run_id,
            Status::Running,
            status,
            detail,
            chrono::Utc::now().timestamp(),
        )?;
        let _ = crate::api::execution::publish(state, inbox);
        return Ok(status != Status::Pending);
    }
    Ok(false)
}

/// Stop markers are checked at launch, including human-rooted pending turns.
/// Policy withdrawal pauses work; it never executes a stale persisted command.
fn launch_rejection(
    state: &AppState,
    request: &super::inbox::Request,
    cfg: &AgentConfig,
) -> anyhow::Result<Option<&'static str>> {
    if let Some(relay) = &request.relay {
        if crate::api::chat::try_relay_is_stopped(state, &relay.root)? {
            return Ok(Some("cascade stopped"));
        }
    }
    if request.delegated()
        && !super::relay::has_budget(
            request.relay.as_ref().unwrap(),
            cfg.resolved(&state.circle_id).max_relay_turns,
        )
    {
        return Ok(Some("relay budget spent"));
    }
    if request.ambient && !cfg.resolved(&state.circle_id).is_ambient(&request.agent) {
        return Ok(Some("ambient participation disabled"));
    }
    if let Some((_, Some((owner, device)))) = Mention::parse(&request.mention_key)
        .as_ref()
        .and_then(|m| m.agent_target())
    {
        if !targets_this_device(state, owner, device) {
            return Ok(Some("recipient device changed"));
        }
    }
    Ok(None)
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
    run_id: &'a str,
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

struct ManagedToken {
    registry: crate::actor_token::ActorTokenRegistry,
    token: String,
}
impl Drop for ManagedToken {
    fn drop(&mut self) {
        self.registry.revoke(&self.token);
    }
}

async fn react(state: &AppState, turn: Turn<'_>, cancel: &CancellationToken) -> anyhow::Result<()> {
    let Turn {
        run_id,
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
    let delivery =
        super::context::build_delivery(state, agent_id, sender, task, resume.as_ref(), message_id);
    let mut prompt = delivery.prompt;
    if ambient {
        prompt.push_str("\n\n");
        prompt.push_str(super::ambient::ambient_instruction());
    }
    let (actor_token, _) = state
        .actor_tokens
        .issue(&state.circle_id, &state.peer_id, agent_id);
    let _token_lease = ManagedToken {
        registry: state.actor_tokens.clone(),
        token: actor_token.clone(),
    };

    let launch = driver::launch_cancellable(
        driver::LaunchRequest {
            run_id: Some(run_id),
            trigger_id: Some(message_id),
            coordination: Some(state.clone()),
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
        },
        cancel,
    );
    tokio::pin!(launch);

    // Long agent runs renew their lease. If this process disappears, peers
    // naturally hide the indicator after the last 45-second lease expires.
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(15));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    heartbeat.tick().await;
    let outcome = loop {
        tokio::select! {
            result = &mut launch => break result?,
            _ = heartbeat.tick() => { state.actor_tokens.renew(&actor_token); publish_agent_activity(
                state,
                agent_id,
                message_id,
                ChatActivityKind::Working,
                true,
            ); },
        }
    };

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
        if let Some(cursor) = &delivery.cursor {
            mark_seen(state, agent_id, cursor);
        }
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
        let posted = crate::api::chat::post_reply(
            state,
            agent_id.to_string(),
            reply.to_string(),
            Vec::new(),
            crate::api::chat::Trigger::AgentReply {
                agent: agent_id.to_string(),
                parent: relay,
            },
            Some(message_id.to_string()),
        );
        posted?;
        if let Some(cursor) = &delivery.cursor {
            mark_seen(state, agent_id, cursor);
        }
    } else {
        tracing::debug!("[agent] `{agent_id}` produced no text reply to post");
        if let Some(cursor) = &delivery.cursor {
            mark_seen(state, agent_id, cursor);
        }
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
        let store = ProposalStore::open(&state.workspace).unwrap();
        let baseline = crate::proposal::snapshot::Snapshot::new(Default::default());
        store.save_snapshot(&baseline).unwrap();
        store.set_baseline(&baseline.id).unwrap();
        (state, dir)
    }

    #[test]
    fn addressed_routes_exclude_ambient_on_every_device() {
        use crate::control::{Author, ChatMessage, CHAT_KEY};
        use yrs::Array;

        for target_peer in ["local", "remote"] {
            let (state, _dir) = test_state("local", "suzy");
            let parent = super::super::relay::mint("root", "human");
            let reply = ChatMessage {
                thread_root: None,
                id: "answer".into(),
                agent_id: "claude".into(),
                text: "Previous answer".into(),
                mentions: vec![],
                ts: 100,
                peer_id: target_peer.into(),
                attachments: vec![],
                relay: Some(super::super::relay::extend(&parent, "claude")),
                author: Author::Agent,
                reply_to: None,
            };
            {
                let mut txn = state.control.transact_mut();
                let chat = txn.get_or_insert_array(CHAT_KEY);
                chat.push_back(
                    &mut txn,
                    Any::String(serde_json::to_string(&reply).unwrap().into()),
                );
            }
            let mut message = ChatMessage {
                thread_root: None,
                id: "next".into(),
                agent_id: "suzy".into(),
                text: "Please explain that answer in more detail".into(),
                mentions: vec![],
                ts: 110,
                peer_id: "human".into(),
                attachments: vec![],
                relay: Some(super::super::relay::mint("next", "human")),
                author: Author::Human,
                reply_to: None,
            };
            let mut cfg = AgentConfig {
                engagement_window_secs: 180,
                ..AgentConfig::default()
            };
            // Legacy follow-ups suppress ambient even when the target is remote.
            let target = resolve_followup(&state, &message, &cfg).unwrap();
            assert_eq!(target.peer_id, target_peer);
            assert!(!ambient_route_allowed(&message, Some(&target)));

            // Explicit replies still route after the time window is disabled.
            cfg.engagement_window_secs = 0;
            message.reply_to = Some("answer".into());
            let target = resolve_followup(&state, &message, &cfg).unwrap();
            assert_eq!(target.peer_id, target_peer);
            assert!(!ambient_route_allowed(&message, Some(&target)));

            message.reply_to = Some("not-synced-yet".into());
            assert!(resolve_followup(&state, &message, &cfg).is_none());
            assert!(!ambient_route_allowed(&message, None));

            // An explicit mention overrides a conflicting reply destination.
            message.mentions = vec!["suzy/other/codex".into()];
            assert!(resolve_followup(&state, &message, &cfg).is_none());
            assert!(!ambient_route_allowed(&message, None));

            message.reply_to = None;
            message.mentions.clear();
            assert!(resolve_followup(&state, &message, &cfg).is_none());
            assert!(ambient_route_allowed(&message, None));
        }
    }

    fn add_chat(state: &AppState, message: &crate::control::ChatMessage) {
        use yrs::Array;
        let mut txn = state.control.transact_mut();
        let chat = txn.get_or_insert_array(crate::control::CHAT_KEY);
        chat.push_back(
            &mut txn,
            Any::String(serde_json::to_string(message).unwrap().into()),
        );
    }

    fn inbox_config() -> AgentConfig {
        let mut cfg = AgentConfig {
            reaction: Reaction::Push,
            ..Default::default()
        };
        cfg.agents.insert(
            "claude".into(),
            super::super::config::AgentCommand::default(),
        );
        cfg.agents.insert(
            "codex".into(),
            super::super::config::AgentCommand::default(),
        );
        cfg.ambient = vec!["claude".into()];
        cfg
    }

    #[test]
    fn reconnect_recovers_an_offline_mention_behind_newer_chat_once() {
        use super::super::inbox::{tests::request, Inbox, Status};
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        drop(inbox); // Target shuts down; the activation boundary remains.
        let mut old = request("historic", "claude").message;
        old.ts = 90;
        let missed = request("offline", "claude").message;
        let mut newer = request("newer-chatter", "claude").message;
        newer.ts = 110;
        newer.mentions.clear();
        add_chat(&state, &old);
        add_chat(&state, &missed);
        add_chat(&state, &newer);
        let inbox = Inbox::open(dir.path(), 200).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let wake = tokio::sync::Notify::new();
        let cfg = inbox_config();
        for _ in 0..3 {
            reconcile_requests_with_config(&state, &handled, &inbox, &wake, &cfg);
        }
        let entries = inbox.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].request.message.id, "offline");
        assert_eq!(entries[0].status, Status::Pending);
    }

    #[test]
    fn explicit_reply_waits_for_missing_parent_and_legacy_markers_stay_suppressed() {
        use super::super::inbox::{tests::request, Inbox};
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let wake = tokio::sync::Notify::new();
        let cfg = inbox_config();
        let legacy = request("old-queue", "claude").message;
        handled.mark_new(&legacy.id, "claude");
        add_chat(&state, &legacy);
        let mut reply = request("reply", "claude").message;
        reply.mentions.clear();
        reply.reply_to = Some("parent".into());
        add_chat(&state, &reply);
        reconcile_requests_with_config(&state, &handled, &inbox, &wake, &cfg);
        assert!(
            inbox
                .entries()
                .iter()
                .all(|e| e.status == super::super::inbox::Status::LegacySuppressed),
            "missing parent must not wake ambient agents"
        );
        let mut parent = request("parent", "claude").message;
        parent.ts = 95;
        parent.mentions.clear();
        parent.peer_id = "local".into();
        parent.author = crate::control::Author::Agent;
        parent.relay = Some(super::super::relay::extend(
            &super::super::relay::mint("root", "sender"),
            "claude",
        ));
        add_chat(&state, &parent);
        reconcile_requests_with_config(&state, &handled, &inbox, &wake, &cfg);
        assert_eq!(inbox.entries().len(), 2);
        assert_eq!(
            inbox.entries()[0].status,
            super::super::inbox::Status::LegacySuppressed
        );
        assert_eq!(inbox.entries()[1].request.message.id, "reply");
    }

    #[test]
    fn replayed_agent_replies_do_not_fan_out_to_every_mention() {
        use super::super::inbox::{tests::request, Inbox};
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let mut message = request("delegation", "claude").message;
        message.author = crate::control::Author::Agent;
        message.mentions = vec!["claude".into(), "codex".into()];
        message.relay = Some(super::super::relay::extend(
            &super::super::relay::mint("root", "sender"),
            "suzent",
        ));
        add_chat(&state, &message);
        let wake = tokio::sync::Notify::new();
        let cfg = inbox_config();
        reconcile_requests_with_config(&state, &handled, &inbox, &wake, &cfg);
        assert_eq!(inbox.entries().len(), 1);
        assert_eq!(inbox.entries()[0].request.agent, "claude");
    }

    #[test]
    fn replay_skips_scoped_self_mentions_using_the_authors_device() {
        use super::super::inbox::{tests::request, Inbox};
        for author_peer in ["local", "remote"] {
            let (state, dir) = test_state("local", "suzy");
            add_member(&state, author_peer, "suzy", "writers-device");
            let inbox = Inbox::open(dir.path(), 100).unwrap();
            let handled = super::super::handled::HandledMentions::load(dir.path());
            let mut message = request("delegation", "claude").message;
            message.author = crate::control::Author::Agent;
            message.peer_id = author_peer.into();
            message.mentions = vec!["suzy/writers-device/claude".into(), "codex".into()];
            message.relay = Some(super::super::relay::extend(
                &super::super::relay::mint("root", "sender"),
                "claude",
            ));
            admit_message(
                &state,
                &handled,
                &inbox,
                &tokio::sync::Notify::new(),
                &inbox_config(),
                &message,
                false,
            );
            assert_eq!(inbox.entries().len(), 1);
            assert_eq!(inbox.entries()[0].request.agent, "codex");
        }
    }

    #[tokio::test]
    async fn stopped_pending_turn_is_cancelled_before_launch_even_under_pull_policy() {
        use super::super::inbox::{tests::request, Inbox, Status};
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        inbox.admit(request("stopped", "claude"), 20, 100).unwrap();
        crate::api::chat::mark_relay_stopped(&state, "stopped").unwrap();
        let mut cfg = inbox_config();
        cfg.reaction = Reaction::Pull;
        assert!(!run_next_with_config(&state, &inbox, &cfg).await.unwrap());
        assert_eq!(inbox.entries()[0].status, Status::Cancelled);
    }

    #[tokio::test]
    async fn unknown_stop_state_defers_execution_without_cancelling() {
        use super::super::inbox::{tests::request, Inbox, Status};
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        inbox.admit(request("pending", "claude"), 20, 100).unwrap();
        let txn = state.control.transact_mut();
        assert!(!run_next_with_config(&state, &inbox, &inbox_config())
            .await
            .unwrap());
        assert_eq!(inbox.entries()[0].status, Status::Pending);
        drop(txn);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pending_turn_uses_current_policy_and_command_and_records_real_completion() {
        use super::super::inbox::{tests::request, Inbox, Status};
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        inbox.admit(request("work", "claude"), 20, 100).unwrap();
        let mut cfg = inbox_config();
        cfg.reaction = Reaction::Pull;
        assert!(!run_next_with_config(&state, &inbox, &cfg).await.unwrap());
        assert_eq!(inbox.entries()[0].status, Status::Pending);
        cfg.reaction = Reaction::Push;
        cfg.agents.get_mut("claude").unwrap().command = vec![
            "/bin/sh".into(),
            "-c".into(),
            "printf ran >> runs.log".into(),
        ];
        assert!(run_next_with_config(&state, &inbox, &cfg).await.unwrap());
        assert_eq!(inbox.entries()[0].status, Status::Completed);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("runs.log")).unwrap(),
            "ran"
        );
        assert!(!run_next_with_config(&state, &inbox, &cfg).await.unwrap());
        inbox.admit(request("failure", "claude"), 20, 101).unwrap();
        cfg.agents.get_mut("claude").unwrap().command =
            vec!["/bin/sh".into(), "-c".into(), "exit 7".into()];
        assert!(run_next_with_config(&state, &inbox, &cfg).await.unwrap());
        assert_eq!(inbox.entries()[1].status, Status::Failed);
        assert!(!run_next_with_config(&state, &inbox, &cfg).await.unwrap());
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
    #[cfg(unix)]
    #[tokio::test]
    async fn different_agents_cross_a_process_barrier_and_finish_independently() {
        use super::super::inbox::{tests::request, Inbox, Status};
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        inbox.admit(request("one", "claude"), 20, 100).unwrap();
        inbox.admit(request("two", "codex"), 20, 101).unwrap();
        let mut cfg = inbox_config();
        let mut a = cfg.agents["claude"].clone();
        a.command = vec![
            "/bin/sh".into(),
            "-c".into(),
            "touch a.ready; while [ ! -f b.ready ]; do sleep 0.01; done; echo a > a.done".into(),
        ];
        let mut b = a.clone();
        b.command[2] =
            "touch b.ready; while [ ! -f a.ready ]; do sleep 0.01; done; echo b > b.done".into();
        cfg.agents.insert("claude".into(), a);
        cfg.agents.insert("codex".into(), b);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (a, b) = tokio::join!(
                run_next_for_agent(&state, &inbox, &cfg, Some("claude")),
                run_next_for_agent(&state, &inbox, &cfg, Some("codex"))
            );
            assert!(a.unwrap());
            assert!(b.unwrap());
        })
        .await
        .expect("serialized agents deadlock at this barrier");
        assert!(inbox
            .entries()
            .iter()
            .all(|e| e.status == Status::Completed));
        assert_eq!(crate::proposal::runs::list(dir.path()).unwrap().len(), 2);
    }

    #[test]
    fn context_cursor_does_not_skip_the_backlog_or_concurrent_messages() {
        let (state, _) = test_state("local", "suzy");
        for i in 0..30 {
            let mut m = super::super::inbox::tests::request(&format!("m{i}"), "claude").message;
            m.text = format!("message {i}");
            add_chat(&state, &m);
        }
        let resume = super::super::memory::Record {
            session_id: "session".into(),
            last_seen_message: "m0".into(),
        };
        let delivered = super::super::context::build_delivery(
            &state,
            "claude",
            "suzy",
            "do it",
            Some(&resume),
            "m29",
        );
        assert_eq!(delivered.cursor.as_deref(), Some("m12"));
        assert!(delivered.prompt.contains("message 1"));
        assert!(delivered.prompt.contains("message 28"));
        assert!(delivered.prompt.contains("Latest room context:"));
        assert!(delivered
            .prompt
            .contains("Omitted lines have NOT been delivered"));
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn a_manual_turn_keeps_automatic_work_pending_until_the_conversation_is_free() {
        use super::super::inbox::{tests::request, Inbox, Status};
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        inbox.admit(request("queued", "claude"), 20, 100).unwrap();
        let mut session = crate::proposal::session::LocalChangeSession::start(
            state.circle_id.clone(),
            "base".into(),
            crate::proposal::session::SessionMode::ManagedProcess,
        );
        session.actor_id = Some("claude".into());
        let mut cfg = inbox_config();
        cfg.agents.get_mut("claude").unwrap().command =
            vec!["/bin/sh".into(), "-c".into(), "exit 0".into()];
        let manual = crate::proposal::runs::RunLease::acquire(dir.path(), session).unwrap();
        assert!(!run_next_with_config(&state, &inbox, &cfg).await.unwrap());
        assert_eq!(inbox.entries()[0].status, Status::Pending);
        drop(manual);
        assert!(run_next_with_config(&state, &inbox, &cfg).await.unwrap());
        assert_eq!(inbox.entries()[0].status, Status::Completed);
    }
    #[tokio::test]
    async fn worker_errors_reach_its_supervisor() {
        use super::super::inbox::{tests::request, Inbox};
        let (mut state, dir) = test_state("local", "suzy");
        let blocked = dir.path().join("not-a-directory");
        std::fs::write(&blocked, "blocked").unwrap();
        state.workspace = blocked;
        let inbox = std::sync::Arc::new(Inbox::open(dir.path(), 100).unwrap());
        inbox.admit(request("m", "claude"), 20, 100).unwrap();
        let worker = spawn_run_worker(
            state,
            inbox,
            std::sync::Arc::new(tokio::sync::Notify::new()),
            CancellationToken::new(),
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), worker)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
    }

    #[tokio::test]
    async fn persistence_failure_reopens_the_owner_without_daemon_restart() {
        use super::super::inbox::{tests::request, Inbox};
        let (state, dir) = test_state("local", "suzy");
        let cancel = CancellationToken::new();
        spawn_reaction(state.clone(), cancel.clone());
        let wait = async {
            loop {
                if let Some(inbox) = state
                    .execution_inbox
                    .read()
                    .unwrap()
                    .as_ref()
                    .and_then(std::sync::Weak::upgrade)
                {
                    break inbox;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        };
        let inbox = tokio::time::timeout(std::time::Duration::from_secs(2), wait)
            .await
            .unwrap();
        let old = std::sync::Arc::downgrade(&inbox);
        let path = Inbox::path(dir.path());
        let original = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(inbox.admit(request("m", "claude"), 20, 100).is_err());
        std::fs::remove_dir(&path).unwrap();
        std::fs::write(&path, original).unwrap();
        drop(inbox);
        let mut message = request("wake", "claude").message;
        message.author = crate::control::Author::System;
        state
            .events
            .send(CircleEvent::MessagePosted { message })
            .unwrap();
        let recovered = async {
            loop {
                let ready = state
                    .execution_inbox
                    .read()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|current| {
                        !current.ptr_eq(&old)
                            && current.upgrade().is_some_and(|inbox| inbox.is_healthy())
                    });
                if ready {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(8), recovered)
            .await
            .unwrap();
        cancel.cancel();
    }
}
