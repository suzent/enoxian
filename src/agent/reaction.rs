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

/// How long the drain waits for a burst to finish before deciding.
const DRAIN_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(750);

/// The longest a steady stream of messages can hold the drain off.
const DRAIN_DEBOUNCE_CAP: std::time::Duration = std::time::Duration::from_secs(3);

/// Parked far enough out that an idle Circle never wakes for it; the reconcile
/// tick is the real heartbeat.
const DRAIN_IDLE: std::time::Duration = std::time::Duration::from_secs(3600);

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
    // `Inbox::open` waits out a departing owner's fork window, so it can block
    // for up to a second. Keep that off a runtime worker: the supervisor above
    // already retries every five seconds, so a real conflict costs nothing here.
    let circle_dir = state.circle_dir.clone();
    let inbox = std::sync::Arc::new(
        tokio::task::spawn_blocking(move || {
            super::inbox::Inbox::open(&circle_dir, chrono::Utc::now().timestamp())
        })
        .await??,
    );
    *state.execution_inbox.write().unwrap() = Some(std::sync::Arc::downgrade(&inbox));
    // Seeded from the current transcript on first run, so switching a Circle to
    // this build decides its whole history as pre-activation rather than
    // collapsing years of chat into a turn nobody asked for.
    let ledger = super::ledger::AmbientLedger::load(
        &state.circle_dir,
        &state
            .transcript()
            .into_iter()
            .map(|m| m.id)
            .collect::<Vec<_>>(),
        chrono::Utc::now().timestamp(),
    );
    let wake = std::sync::Arc::new(tokio::sync::Notify::new());
    let worker_token = token.child_token();
    let _cancel_worker = worker_token.clone().drop_guard();
    let mut worker = spawn_run_worker(state.clone(), inbox.clone(), wake.clone(), worker_token);
    // Only the follow-up window still distinguishes a live post from a replayed
    // one. Ambient used to as well, on the author's clock; see `ledger`.
    let live_since = chrono::Utc::now().timestamp() - 2;
    let mut reconcile = tokio::time::interval(std::time::Duration::from_secs(5));
    reconcile.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // A burst of messages resolves as one set rather than as N races, so three
    // lines typed in four seconds produce one turn on the last of them instead
    // of two deaths on the length floor and one turn with no idea the other two
    // were part of the same thought (§2.2). The cap bounds how long a steady
    // stream of chatter can hold the drain off.
    let mut dirty_since: Option<tokio::time::Instant> = None;
    let debounce = tokio::time::sleep(DRAIN_IDLE);
    tokio::pin!(debounce);
    loop {
        tokio::select! {
            _ = token.cancelled() => break,
            result = &mut worker => {
                result??;
                anyhow::bail!("execution worker stopped unexpectedly; recovering owner");
            },
            _ = reconcile.tick() => {
                if let Err(error) = crate::proposal::runs::release_finished_locks(&state) { tracing::debug!("lock cleanup deferred: {error}"); }
                reconcile_requests(&state, &handled, &inbox, &ledger, &wake);
                // The heartbeat drain. Covers a daemon that starts holding a
                // backlog and then hears nothing, which no event would wake.
                dirty_since = None;
                drain_ambient(&state, &handled, &inbox, &ledger, &wake, &AgentConfig::load());
            }
            _ = &mut debounce, if dirty_since.is_some() => {
                dirty_since = None;
                debounce.as_mut().reset(tokio::time::Instant::now() + DRAIN_IDLE);
                drain_ambient(&state, &handled, &inbox, &ledger, &wake, &AgentConfig::load());
            }
            evt = events.recv() => match evt {
                Ok(CircleEvent::MessagePosted { message }) => {
                    let cfg = AgentConfig::load();
                    admit_message(&state, &handled, &inbox, &ledger, &wake, &cfg, &message,
                        message.ts >= live_since);
                    let started = *dirty_since.get_or_insert_with(tokio::time::Instant::now);
                    let now = tokio::time::Instant::now();
                    debounce.as_mut().reset((now + DRAIN_DEBOUNCE).min(started + DRAIN_DEBOUNCE_CAP));
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
                    reconcile_requests(&state, &handled, &inbox, &ledger, &wake);
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
    ledger: &super::ledger::AmbientLedger,
    wake: &tokio::sync::Notify,
) {
    reconcile_requests_with_config(state, handled, inbox, ledger, wake, &AgentConfig::load());
}

fn reconcile_requests_with_config(
    state: &AppState,
    handled: &super::handled::HandledMentions,
    inbox: &super::inbox::Inbox,
    ledger: &super::ledger::AmbientLedger,
    wake: &tokio::sync::Notify,
    cfg: &AgentConfig,
) {
    let mut history = state.transcript();
    for (message_id, mention_key) in handled.entries() {
        let Some(message) = history.iter().find(|m| m.id == message_id) else {
            continue;
        };
        let ambient = mention_key.starts_with("~ambient:");
        let target = mention_key
            .strip_prefix("~ambient:")
            .unwrap_or(&mention_key);
        let Some(mention) = Mention::parse(target) else {
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
            ambient,
            // A legacy marker records that a turn happened, not who else saw it.
            co_listeners: Vec::new(),
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
            admit_message(state, handled, inbox, ledger, wake, cfg, &message, false);
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

#[allow(clippy::too_many_arguments)]
fn admit_message(
    state: &AppState,
    handled: &super::handled::HandledMentions,
    inbox: &super::inbox::Inbox,
    ledger: &super::ledger::AmbientLedger,
    wake: &tokio::sync::Notify,
    cfg: &AgentConfig,
    message: &crate::control::ChatMessage,
    live: bool,
) {
    // Was this already in the room when this device started listening? Asked of
    // the ledger rather than by comparing the author's clock against ours
    // (§2.4). The old `message.ts < inbox.activated_at()` did not just lose
    // ambient turns: a peer whose clock sat behind this device's activation
    // had its *addressed mentions* dropped here, silently, for as long as the
    // Circle existed.
    if ledger.predates_activation(&message.id) || message.author == crate::control::Author::System {
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
                co_listeners: Vec::new(),
                addressed_here: scope.is_some(),
            },
        );
    }
    let engagement = if live || message.reply_to.is_some() {
        resolve_followup(state, message, cfg)
    } else {
        None
    };
    // Ambient no longer happens here. It is decided by `drain_ambient` over the
    // set of messages this device has never decided about, which is why the two
    // wall-clock gates that used to stand at this point are gone (§1.2).
    if !ambient_route_allowed(message, engagement.as_ref())
        && message.author == crate::control::Author::Human
        && engagement.is_none()
        && !mentions_an_agent(message)
        && !cfg.resolved(&state.circle_id).ambient.is_empty()
    {
        // The one route that still drops a message with nobody claiming it: a
        // reply pointing at something no agent posted resolves to no engagement
        // and is not offered to the room either (§6).
        note(
            state,
            &message.id,
            None,
            "this replies to a message no agent posted, so no agent was routed",
        );
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
                co_listeners: Vec::new(),
                addressed_here: true,
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
    /// Every agent offered this same message, when `ambient` — this one
    /// included. Passed into the prompt so a listener knows it is not alone.
    co_listeners: Vec<String>,
    /// Is this dispatch unambiguously meant for *this* device?
    ///
    /// True for a device-scoped mention, a follow-up, and an ambient offer.
    /// False for a bare `@claude`, which fans out to every device: a machine
    /// that does not configure `claude` is not at fault for ignoring it, and
    /// recording a diagnostic for that would bury the real ones.
    addressed_here: bool,
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
    // Both of these are ordinary and silent, and between them they explain most
    // reports of "the agent wasn't triggered" — so say why, locally (§4.2).
    if settings.reaction != Reaction::Push {
        if req.addressed_here {
            note(
                state,
                &req.message.id,
                Some(req.agent),
                "this device is set to pull, so it launches nothing on its own",
            );
        }
        return;
    }
    if cfg.resolve(req.agent).is_none() {
        if req.addressed_here {
            note(
                state,
                &req.message.id,
                Some(req.agent),
                "no agent by this name is configured on this device",
            );
        }
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
        co_listeners: req.co_listeners,
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
            // It ran after all; an earlier "nobody was eligible" must not sit
            // in the panel beside the turn that superseded it.
            state.admission_log.clear_message(&entry.request.message.id);
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

/// Decide every message this device has not yet decided about (§2.2).
///
/// Ambient admission used to run once per `MessagePosted` event, gated on the
/// message being under thirty seconds old by the *author's* clock. Draining
/// instead of reacting is what makes the invariant in [`super::ledger`]
/// expressible: eligibility is "never decided here", which no clock can get
/// wrong, and a burst of messages resolves as one set rather than as N
/// independent races.
///
/// Returns the number of messages decided, for the caller's logging.
fn drain_ambient(
    state: &AppState,
    handled: &super::handled::HandledMentions,
    inbox: &super::inbox::Inbox,
    ledger: &super::ledger::AmbientLedger,
    wake: &tokio::sync::Notify,
    cfg: &AgentConfig,
) -> usize {
    use super::ledger::Decision;
    let now = chrono::Utc::now().timestamp();
    let history = state.transcript();
    // CRDT insertion order, which every peer agrees on and which involves no
    // clock at all. Deliberately not `ts` order: sorting a backlog by the
    // authors' clocks is the same mistake one layer down.
    let entries = inbox.entries();
    let mut undecided: Vec<&crate::control::ChatMessage> = Vec::new();
    // A message whose every attempt failed is not answered, and the ledger
    // saying `offered` does not make it so. It rejoins the candidates — but
    // separately from never-seen messages, because it has already won a tail
    // slot once and must not have to win another (§3.2).
    let mut reopened: Vec<&crate::control::ChatMessage> = Vec::new();
    for message in history.iter() {
        match ledger.decision(&message.id) {
            None => undecided.push(message),
            Some(super::ledger::Decision::Offered) => {
                if unanswered_attempts(&entries, &message.id).is_some() {
                    reopened.push(message);
                }
            }
            Some(_) => {}
        }
    }
    if undecided.is_empty() && reopened.is_empty() {
        return 0;
    }
    // Which agents read the room *here*. An agent can be ambient in a working
    // Circle and silent in a social one, so the answer is per Circle.
    let settings = cfg.resolved(&state.circle_id);
    // Resolve to the spelling `[agents.*]` uses, not the one the ambient list
    // happens to be written in. `is_ambient` has always compared without regard
    // to case while this filter did not, so `ambient = ["Claude"]` against
    // `[agents.claude]` was silently inert — and everything downstream, from
    // `cfg.resolve` to the `~ambient:` dedup key, still wants the exact key (§6).
    let listeners: Vec<String> = settings
        .ambient
        .iter()
        .filter_map(|name| {
            cfg.agents
                .keys()
                .find(|configured| configured.eq_ignore_ascii_case(name))
                .cloned()
        })
        .collect();
    if listeners.is_empty() {
        // Nobody is listening, so nothing is undecided in any meaningful sense.
        // Recording them keeps the drain O(new messages) instead of rescanning
        // the whole transcript forever, and turning a listener on later should
        // not retroactively answer everything said while none was configured.
        for message in &undecided {
            ledger.record(&message.id, Decision::NoListener, now);
        }
        return undecided.len();
    }
    // Retry the failures before spending anything on new messages: a message
    // the room is still waiting on outranks one nobody has looked at yet.
    let max_attempts = settings.ambient_max_attempts.max(1);
    for message in &reopened {
        let Some(state_of) = unanswered_attempts(&entries, &message.id) else {
            continue;
        };
        // Two separate ceilings, because they bound two different things: how
        // many agents may burn a real turn on one message, and how many times a
        // restart may put back a turn that never ran.
        let spent = match (
            state_of.executed >= max_attempts,
            state_of.expired >= EXPIRED_REQUEUE_LIMIT,
        ) {
            (true, _) => Some("every attempt failed"),
            (_, true) => Some("this device kept restarting before it could run"),
            _ => None,
        };
        if let Some(reason) = spent {
            ledger.settle(&message.id, Decision::Skipped(reason.into()), now);
            note(state, &message.id, None, reason);
            announce_unanswered(state, &message.id, reason);
            continue;
        }
        let untried: Vec<String> = listeners
            .iter()
            .filter(|name| {
                !state_of
                    .offered
                    .iter()
                    .any(|t| t.eq_ignore_ascii_case(name))
            })
            .cloned()
            .collect();
        if !untried.is_empty() {
            offer_to_listeners(
                state, handled, inbox, ledger, wake, cfg, &untried, message, now,
            );
            continue;
        }
        // Everyone has had a go. One case is still worth another: a turn that
        // expired without ever starting — a daemon restart while it queued —
        // was not an attempt by that agent at anything, so offering it again is
        // not a retry. The inbox dedups on (message, agent), so this has to go
        // through `retry` rather than a fresh admission.
        if let Some(run_id) = state_of.never_started {
            match inbox.retry(&run_id, settings.max_relay_turns, now) {
                Ok(_) => {
                    wake.notify_one();
                    continue;
                }
                Err(error) => tracing::debug!("[agent] could not requeue {run_id}: {error}"),
            }
        }
        let reason = "every agent reading this room tried and could not answer";
        ledger.settle(&message.id, Decision::Skipped(reason.into()), now);
        note(state, &message.id, None, reason);
        announce_unanswered(state, &message.id, reason);
    }

    // The cheap gates run before the tail is chosen, so a backlog ending in
    // "ok thanks" does not spend its one turn there while a real question
    // sits behind it.
    let mut eligible: Vec<&crate::control::ChatMessage> = Vec::new();
    for message in undecided {
        // Is this part of an exchange somebody is already having with an agent?
        // The check used to sit on the admission path, which is where ambient
        // used to be decided; moving the decision here without it let an
        // explicit reply wake a listener *as well as* the agent it was aimed at.
        match addressing(&history, state, message, cfg) {
            // Somebody is already talking to an agent; the room is not invited.
            Addressing::ToAnAgent => {
                ledger.record(
                    &message.id,
                    Decision::Skipped("part of an exchange with an agent".into()),
                    now,
                );
                continue;
            }
            // The message it replies to has not synced. It may turn out to be
            // an agent's, so deciding now would be a guess. Leaving it
            // undecided means the next drain looks again — which is the whole
            // point of deciding from a ledger rather than from a clock.
            Addressing::Unknown => continue,
            Addressing::ToTheRoom => {}
        }
        match super::ambient::skip_reason(message, mentions_an_agent(message)) {
            Some(reason) => {
                ledger.record(&message.id, Decision::Skipped(reason.into()), now);
                // Only for messages a person wrote. "not human-authored" is the
                // rule that makes ambient terminate (§2.1), it fires on every
                // agent reply, and nobody has ever wondered why their agent did
                // not answer itself.
                if message.author == crate::control::Author::Human {
                    note(state, &message.id, None, reason);
                }
            }
            None => eligible.push(message),
        }
    }
    // Collapse to the tail, never discard it. However late a message arrives,
    // and however long the daemon was down, the most recent thing said in the
    // room is read (§2.3).
    let carried = settings.ambient_backlog_tail.min(eligible.len());
    let collapsed = eligible.len() - carried;
    for message in eligible.drain(..collapsed) {
        ledger.record(&message.id, Decision::Backlog, now);
        note(
            state,
            &message.id,
            None,
            "part of a backlog; only its most recent messages were read",
        );
    }
    let decided = collapsed + eligible.len() + reopened.len();
    for message in eligible {
        offer_to_listeners(
            state, handled, inbox, ledger, wake, cfg, &listeners, message, now,
        );
    }
    decided
}

/// What happened to a message this device offered, when the room is still
/// waiting on it.
///
/// `None` means the message is settled and must not be reopened:
///
/// - `Completed` — answered, including a PASS, which is a considered answer.
/// - `Pending` / `Running` — not finished failing yet.
/// - `Cancelled` — a deliberate withdrawal, not a failure. "Another agent
///   answered first", "cascade stopped" and "ambient participation disabled"
///   are all decisions, and retrying a decision would undo it.
struct Unanswered {
    /// Every agent with an entry for this message, in admission order —
    /// whether it ran or expired before it could.
    ///
    /// This is what a fresh offer must skip, and the reason is mechanical
    /// rather than principled: the inbox dedups on `(message, agent)`, so
    /// re-admitting one of these is silently dropped. Putting an agent back
    /// goes through `Inbox::retry`, not a new admission.
    offered: Vec<String>,
    /// Runs that actually executed and did not produce an answer.
    ///
    /// This is what `ambient_max_attempts` bounds, and it deliberately excludes
    /// runs that expired without starting. Counting every entry instead made
    /// the budget unspendable the moment `ambient_responders` reached it: two
    /// listeners offered in the opening round are two entries, so one restart
    /// left `2 >= 2` and the message was settled as "every attempt failed"
    /// having never run at all.
    executed: usize,
    /// Runs that expired without ever starting, and the first one's id.
    ///
    /// Distinct from a failure: nothing was tried, so offering the same agent
    /// again is not a retry of anything. Bounded separately by
    /// [`EXPIRED_REQUEUE_LIMIT`] — a daemon restarting in a loop expires the
    /// requeue it just made, which without a cap of its own is forever.
    ///
    /// `Interrupted` is deliberately not here — that run was executing, may
    /// have done visible work, and the inbox already records its retry as
    /// suppressed.
    expired: usize,
    never_started: Option<String>,
}

/// How many times a turn killed before it started may be put back.
///
/// Not user-facing: it bounds a pathology (a daemon restart loop), not a
/// policy. Three is enough to survive an upgrade or a crash-restart without
/// letting a machine that cannot stay up requeue the same turn indefinitely.
const EXPIRED_REQUEUE_LIMIT: usize = 3;

fn unanswered_attempts(entries: &[super::inbox::Entry], message_id: &str) -> Option<Unanswered> {
    use super::inbox::Status;
    let mut out = Unanswered {
        offered: Vec::new(),
        executed: 0,
        expired: 0,
        never_started: None,
    };
    for entry in entries
        .iter()
        .filter(|e| e.request.ambient && e.request.message.id == message_id)
    {
        match entry.status {
            Status::Completed | Status::Pending | Status::Running | Status::Cancelled => {
                return None
            }
            status => {
                match status {
                    Status::Expired => {
                        out.expired += 1;
                        if out.never_started.is_none() {
                            out.never_started = Some(entry.run_id.clone());
                        }
                    }
                    // Ran, and produced no answer. Only these spend the budget.
                    _ => out.executed += 1,
                }
                if !out
                    .offered
                    .iter()
                    .any(|a| a.eq_ignore_ascii_case(&entry.request.agent))
                {
                    out.offered.push(entry.request.agent.clone());
                }
            }
        }
    }
    (out.executed + out.expired > 0).then_some(out)
}

/// Offer one eligible message to this device's listeners./// Offer one eligible message to this device's listeners.
///
/// The ledger entry is written before any dispatch, so a panic or a crash
/// between the two costs the room one turn rather than replaying the selection
/// on the next drain.
#[allow(clippy::too_many_arguments)]
fn offer_to_listeners(
    state: &AppState,
    handled: &super::handled::HandledMentions,
    inbox: &super::inbox::Inbox,
    ledger: &super::ledger::AmbientLedger,
    wake: &tokio::sync::Notify,
    cfg: &AgentConfig,
    listeners: &[String],
    message: &crate::control::ChatMessage,
    now: i64,
) {
    use super::ledger::Decision;
    let settings = cfg.resolved(&state.circle_id);
    let history = state.transcript();
    let previous = inbox.entries();
    let mut available: Vec<String> = listeners
        .iter()
        // `now` is this device's clock deciding about its own recent past —
        // "did this agent just speak here" — rather than a comparison against
        // the author's.
        .filter(|agent| !super::ambient::spoke_recently(&history, agent, now))
        .filter(|agent| !handled.contains(&message.id, &format!("~ambient:{agent}")))
        .cloned()
        .collect();
    if available.is_empty() {
        ledger.record(
            &message.id,
            Decision::Skipped("every listener had just spoken".into()),
            now,
        );
        note(
            state,
            &message.id,
            None,
            match listeners.len() {
                1 => "the only agent reading this room had just spoken",
                _ => "every agent reading this room had just spoken",
            },
        );
        return;
    }
    // Least recently offered first, so slow providers and PASS count as turns.
    // Stable ties preserve the configured order only for never-offered agents.
    available.sort_by_key(|agent| {
        previous
            .iter()
            .rposition(|e| e.request.ambient && e.request.agent == *agent)
    });
    let max = settings.ambient_responders.min(available.len());
    // How many listeners this particular message gets, when the count is set
    // to vary. Derived from the message id rather than from a running count of
    // past offers: the inbox that count came from is trimmed and rebuilt across
    // restarts, so the "cycle" the setting promises was not reproducible and
    // the same message could be answered by a different number of agents
    // depending on when the daemon last started (§6).
    let count = if settings.ambient_rotate_count && max > 0 {
        1 + (fnv1a(&message.id) as usize) % max
    } else {
        max
    };
    // Fix the selection before dispatching any of it: each listener's prompt
    // names the others, which cannot be known while the list is still being
    // consumed one agent at a time.
    let selected: Vec<String> = available.into_iter().take(count).collect();
    ledger.record(&message.id, Decision::Offered, now);
    for agent in &selected {
        dispatch(
            state,
            handled,
            inbox,
            wake,
            cfg,
            DispatchRequest {
                agent,
                mention_key: &format!("~ambient:{agent}"),
                task: message.text.clone(),
                message,
                // An ambient turn is rooted in the human message it reads, so a
                // hand-off from it spends that message's budget (§3.8).
                relay: Some(super::relay::mint(&message.id, &message.peer_id)),
                implicit: false,
                ambient: true,
                co_listeners: selected.clone(),
                addressed_here: true,
            },
        );
    }
}

/// FNV-1a over the message id.
///
/// Wanted for one thing only: a stable number per message, agreed on by any
/// device and any daemon run. `DefaultHasher` is explicitly not stable across
/// releases, and nothing here needs collision resistance.
fn fnv1a(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

/// Does this message address an agent (rather than a person or nobody)?
fn mentions_an_agent(message: &crate::control::ChatMessage) -> bool {
    message.mentions.iter().any(|m| {
        Mention::parse(m)
            .and_then(|parsed| parsed.agent_target().map(|_| ()))
            .is_some()
    })
}

/// Who a message that names no agent is talking to.
enum Addressing {
    /// Nobody in particular: the room may be offered it.
    ToTheRoom,
    /// An agent, by mention or by replying to one of its posts.
    ToAnAgent,
    /// It replies to a message this device has not received yet, so there is
    /// no way to tell which of the above it is.
    Unknown,
}

/// Classify a message for the drain.
///
/// Replying to a *person* used to land here as addressed and be dropped by both
/// routes — no engagement resolves from a human's post, and ambient refused
/// anything carrying a `reply_to` at all — so quoting a colleague to ask "does
/// anyone know how this works?" guaranteed no agent would ever see it (§6). A
/// reply to a person is still a message in a room that names no agent, and the
/// cheap gates apply to it like any other.
fn addressing(
    history: &[crate::control::ChatMessage],
    state: &AppState,
    message: &crate::control::ChatMessage,
    cfg: &AgentConfig,
) -> Addressing {
    if resolve_followup_in(history, state, message, cfg).is_some() {
        return Addressing::ToAnAgent;
    }
    match &message.reply_to {
        None => Addressing::ToTheRoom,
        // Present, and it resolved to no agent above, so it is a person's.
        Some(parent) if history.iter().any(|m| &m.id == parent) => Addressing::ToTheRoom,
        Some(_) => Addressing::Unknown,
    }
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
    resolve_followup_in(&state.transcript(), state, message, cfg)
}

/// As [`resolve_followup`], over a transcript the caller already holds.
///
/// The drain resolves this for every candidate message, and re-reading the
/// control doc once per message would make a backlog quadratic.
fn resolve_followup_in(
    all: &[crate::control::ChatMessage],
    state: &AppState,
    message: &crate::control::ChatMessage,
    cfg: &AgentConfig,
) -> Option<super::engagement::Engagement> {
    if !super::engagement::is_followup_candidate(message, mentions_an_agent(message)) {
        return None;
    }
    let mut history = all.to_vec();
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
                co_listeners: &request.co_listeners,
            },
            cancel,
        )
        .await;
        let (status, detail) = match result {
            Ok(detail) => (Status::Completed, detail),
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
        // An unaddressed turn's failure is not the end of the message: the
        // next drain hands it to another listener. `drain_ambient` announces
        // if and when it runs out of them.
        if status == Status::Failed && !request.ambient {
            announce_failure(
                state,
                request,
                detail.as_deref().unwrap_or("no reason was reported"),
            );
        }
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

/// One line per agent per window, however badly it is failing. A misconfigured
/// adapter in a busy room would otherwise narrate every message.
const ANNOUNCE_THROTTLE_SECS: i64 = 120;

/// How much of the transcript tail to search for a prior announcement.
const ANNOUNCE_SCAN: usize = 50;

/// The opening of a failure announcement, and how we recognise our own.
fn failure_prefix(agent: &str) -> String {
    format!("{agent} could not answer")
}

/// The opening used when a message the room was waiting on runs out of agents.
const UNANSWERED_PREFIX: &str = "No agent could answer";

/// Post one line in the room, unless it would be noise.
///
/// Suppressed when another agent has already replied to the message, and
/// throttled so a misconfigured adapter in a busy room cannot narrate every
/// message. `prefix` is both the opening of the line and how a prior
/// announcement of the same kind is recognised.
fn announce(state: &AppState, message_id: &str, prefix: &str, text: String) {
    let now = chrono::Utc::now().timestamp();
    if answered_by_an_agent(state, message_id) {
        return;
    }
    let transcript = state.transcript();
    let recent = &transcript[transcript.len().saturating_sub(ANNOUNCE_SCAN)..];
    if recent.iter().any(|m| {
        m.author == crate::control::Author::System
            && now - m.ts <= ANNOUNCE_THROTTLE_SECS
            && m.text.starts_with(prefix)
    }) {
        return;
    }
    if let Err(error) = crate::api::chat::post_reply(
        state,
        "system".into(),
        text,
        Vec::new(),
        crate::api::chat::Trigger::System,
        Some(message_id.to_string()),
    ) {
        tracing::debug!("[agent] could not announce a failed turn: {error}");
    }
}

/// Say that a message nobody addressed has run out of agents to try (§3.2).
///
/// The counterpart to [`announce_failure`], which covers the addressed case. An
/// individual ambient failure is *not* announced: it hands the message to the
/// next listener, so saying "no reply is coming" at that point would be wrong.
/// Only giving up is final.
fn announce_unanswered(state: &AppState, message_id: &str, reason: &str) {
    announce(
        state,
        message_id,
        UNANSWERED_PREFIX,
        format!(
            "{UNANSWERED_PREFIX} this — {reason}. Mention an agent directly to ask one for a \
             reply, or retry from Agent activity."
        ),
    );
}

/// Say in the room that an addressed turn failed and no reply is coming (§4.1).
///
/// The execution panel already records this durably, but the panel is a side
/// surface with a 3-second poll, and the only in-chat signal — a `Skipped`
/// [`ChatActivity`] — expires after 45 seconds. A turn that dies half an hour
/// after it was asked for therefore blinks at nobody and leaves the transcript
/// looking as though the message was simply ignored.
///
/// Deliberately narrow. Not for PASS, which is an answer; not for the cheap
/// gates, which are policy; not for `Cancelled` or `Expired`, which are this
/// device withdrawing rather than failing; and not for an *unaddressed* turn,
/// whose failure hands the message to the next listener rather than ending it —
/// see [`announce_unanswered`] for where that case is reported instead. Only
/// "you asked, it broke, nothing is coming".
fn announce_failure(state: &AppState, request: &super::inbox::Request, reason: &str) {
    let prefix = failure_prefix(&request.agent);
    let text =
        format!("{prefix} — {reason}. No reply was posted; you can try again from Agent activity.");
    announce(state, &request.message.id, &prefix, text);
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
    // An unaddressed turn can sit behind a device permit for minutes while
    // another listener answers. Nobody asked this one for anything, so a second
    // take on a point already made is worse than silence — and this is the last
    // moment the queue can tell. An addressed turn is not covered: if you named
    // this agent, someone else replying does not discharge the request.
    if request.ambient && answered_by_an_agent(state, &request.message.id) {
        return Ok(Some("another agent answered first"));
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

/// Has some agent already replied to this message?
///
/// `react` posts every agent reply with `reply_to` set to the message that woke
/// it, so a direct child by an agent is the whole test.
fn answered_by_an_agent(state: &AppState, message_id: &str) -> bool {
    state.transcript().iter().any(|m| {
        m.reply_to.as_deref() == Some(message_id) && m.author == crate::control::Author::Agent
    })
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
    /// The other agents shown the same message, on an unaddressed turn.
    co_listeners: &'a [String],
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

async fn react(
    state: &AppState,
    turn: Turn<'_>,
    cancel: &CancellationToken,
) -> anyhow::Result<Option<String>> {
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
        co_listeners,
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
    let framing = if ambient {
        super::context::Framing::Overheard
    } else {
        super::context::Framing::Addressed
    };
    let delivery = super::context::build_delivery(
        state,
        agent_id,
        sender,
        task,
        resume.as_ref(),
        message_id,
        framing,
    );
    let mut prompt = delivery.prompt;
    if ambient {
        prompt.push_str("\n\n");
        prompt.push_str(&super::ambient::ambient_instruction(co_listeners, agent_id));
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
            withheld: &delivery.withheld,
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
            mark_seen(state, agent_id, cursor, &delivery.delivered);
        }
        publish_agent_activity_detailed(
            state,
            agent_id,
            message_id,
            ChatActivityKind::Skipped,
            true,
            Some("nothing to add".to_string()),
        );
        return Ok(Some("No reply needed".into()));
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
            mark_seen(state, agent_id, cursor, &delivery.delivered);
        }
    } else {
        tracing::debug!("[agent] `{agent_id}` produced no text reply to post");
        if let Some(cursor) = &delivery.cursor {
            mark_seen(state, agent_id, cursor, &delivery.delivered);
        }
    }
    publish_agent_activity(
        state,
        agent_id,
        message_id,
        ChatActivityKind::Working,
        false,
    );
    Ok(None)
}

/// Remember the last chat line this agent has seen — its own reply, or the
/// mention it just handled when it said nothing — so the next turn carries only
/// what the room said in between. `delivered` records the lines this prompt
/// showed *ahead* of that cursor, which the next turn subtracts. Best-effort: a
/// failure here costs the next prompt a few already-seen lines, not correctness.
fn mark_seen(state: &AppState, agent_id: &str, message_id: &str, delivered: &[String]) {
    if let Err(e) =
        super::memory::save_seen(&state.circle_dir, agent_id, message_id, delivered.to_vec())
    {
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
/// Record why this device did not act on a message (§4.2).
///
/// Diagnostic only, and local only. Costs a mutex and a short scan, so it is
/// safe on any path — including the reconcile tick, which re-derives the same
/// decision every five seconds and is collapsed by [`decisions::AdmissionLog`].
fn note(state: &AppState, message_id: &str, agent: Option<&str>, reason: &str) {
    tracing::debug!("[agent] no turn for {message_id} ({agent:?}): {reason}");
    state
        .admission_log
        .record(message_id, agent, reason, chrono::Utc::now().timestamp());
}

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

    /// How long a test waits on a real process or a background task before it
    /// calls the thing hung.
    ///
    /// Every use below is a *liveness* bound, not a speed one: the work it
    /// guards either completes or blocks forever — a serialized barrier never
    /// resolves, an inbox that is never republished never arrives — so raising
    /// the value weakens no assertion. All it costs is how long a genuinely
    /// broken run takes to fail. It is therefore set well past what a heavily
    /// loaded machine needs (a parallel test suite, a shared CI runner);
    /// tightening it buys nothing and brings the flakes back. Timeouts that
    /// assert something must *not* finish are the opposite case and must stay
    /// short — there are none in this module.
    const LIVENESS: std::time::Duration = std::time::Duration::from_secs(60);

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
        let mut old = request("historic", "claude").message;
        old.ts = 90;
        add_chat(&state, &old);
        // The device activates with `historic` already in the room. That, and
        // not a timestamp, is what the boundary records.
        let ledger =
            super::super::ledger::AmbientLedger::load(dir.path(), &["historic".to_string()], 100);
        drop(ledger);
        drop(inbox); // Target shuts down; the activation boundary remains.

        let missed = request("offline", "claude").message;
        let mut newer = request("newer-chatter", "claude").message;
        newer.ts = 110;
        newer.mentions.clear();
        add_chat(&state, &missed);
        add_chat(&state, &newer);
        let inbox = Inbox::open(dir.path(), 200).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        // A restart reloads the boundary from disk rather than deriving a new
        // one, so everything that arrived while the device was down is new.
        let ledger = super::super::ledger::AmbientLedger::load(
            dir.path(),
            &state
                .transcript()
                .into_iter()
                .map(|m| m.id)
                .collect::<Vec<_>>(),
            200,
        );
        let wake = tokio::sync::Notify::new();
        let cfg = inbox_config();
        for _ in 0..3 {
            reconcile_requests_with_config(&state, &handled, &inbox, &ledger, &wake, &cfg);
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
        let ledger = ledger_at_startup(dir.path());
        let wake = tokio::sync::Notify::new();
        let cfg = inbox_config();
        let legacy = request("old-queue", "claude").message;
        handled.mark_new(&legacy.id, "claude");
        add_chat(&state, &legacy);
        let mut reply = request("reply", "claude").message;
        reply.mentions.clear();
        reply.reply_to = Some("parent".into());
        add_chat(&state, &reply);
        reconcile_requests_with_config(&state, &handled, &inbox, &ledger, &wake, &cfg);
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
        reconcile_requests_with_config(&state, &handled, &inbox, &ledger, &wake, &cfg);
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
        let ledger = ledger_at_startup(dir.path());
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
        reconcile_requests_with_config(&state, &handled, &inbox, &ledger, &wake, &cfg);
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
                &ledger_at_startup(dir.path()),
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
        tokio::time::timeout(LIVENESS, async {
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
            ..Default::default()
        };
        let delivered = super::super::context::build_delivery(
            &state,
            "claude",
            "suzy",
            "do it",
            Some(&resume),
            "m29",
            super::super::context::Framing::Addressed,
        );
        assert_eq!(delivered.cursor.as_deref(), Some("m12"));
        assert!(delivered.prompt.contains("message 1"));
        assert!(delivered.prompt.contains("message 28"));
        assert!(delivered.prompt.contains("Latest room context:"));
        assert!(delivered
            .prompt
            .contains("Omitted lines have NOT been delivered"));
    }

    /// Chat helper that lets a test choose the author and the device it came
    /// from — the two things that decide whether a line is the agent's own.
    fn say(state: &AppState, id: &str, author: &str, peer: &str, text: &str) {
        let mut m = super::super::inbox::tests::request(id, "claude").message;
        m.agent_id = author.into();
        m.peer_id = peer.into();
        m.text = text.into();
        m.mentions = Vec::new();
        add_chat(state, &m);
    }

    /// A resumed ACP session already holds the agent's own turns, so quoting
    /// them back as "the room" shows the agent its own words twice.
    #[test]
    fn a_resumed_prompt_omits_this_agents_own_posts_but_keeps_its_namesakes() {
        let (state, _) = test_state("local", "suzy");
        say(&state, "m0", "human", "sender", "start here");
        say(&state, "m1", "claude", "local", "my own earlier reply");
        say(
            &state,
            "m2",
            "claude",
            "other-device",
            "a namesake on another box",
        );
        say(&state, "m3", "human", "sender", "and the room moved on");
        let resume = super::super::memory::Record {
            session_id: "session".into(),
            last_seen_message: "m0".into(),
            ..Default::default()
        };
        let delivered = super::super::context::build_delivery(
            &state,
            "claude",
            "suzy",
            "do it",
            Some(&resume),
            "m3",
            super::super::context::Framing::Addressed,
        );
        assert!(
            !delivered.prompt.contains("my own earlier reply"),
            "the resumed session already holds this: {}",
            delivered.prompt
        );
        assert!(delivered.prompt.contains("a namesake on another box"));
        // A fresh session has no such memory, so nothing is withheld from it.
        let fresh = super::super::context::build_delivery(
            &state,
            "claude",
            "suzy",
            "do it",
            None,
            "m3",
            super::super::context::Framing::Addressed,
        );
        assert!(fresh.prompt.contains("my own earlier reply"));
    }

    /// Withholding the agent's own posts bets on the resumed session holding
    /// them. When `session/load` fails that bet is lost, so the recovery
    /// context has to put back what the prompt left out — the cursor advances
    /// past those lines either way.
    #[test]
    fn a_failed_resume_restores_what_the_prompt_withheld() {
        let (state, _) = test_state("local", "suzy");
        say(&state, "m0", "human", "sender", "start here");
        say(
            &state,
            "m1",
            "claude",
            "local",
            "a decision only I recorded",
        );
        for i in 2..20 {
            say(
                &state,
                &format!("m{i}"),
                "human",
                "sender",
                &format!("filler {i}"),
            );
        }
        let delivered = super::super::context::build_delivery(
            &state,
            "claude",
            "suzy",
            "do it",
            Some(&super::super::memory::Record {
                session_id: "session".into(),
                last_seen_message: "m0".into(),
                ..Default::default()
            }),
            "m19",
            super::super::context::Framing::Addressed,
        );
        assert!(!delivered.prompt.contains("a decision only I recorded"));
        assert_eq!(delivered.withheld, vec!["m1".to_string()]);

        // m1 has long fallen out of the room-history window, so without the
        // withheld list the restarted session would never see it again.
        let recovery =
            super::super::context::recovery_context(&state, "claude", &delivered.withheld);
        assert!(
            recovery.contains("a decision only I recorded"),
            "recovery must put back what the resume assumption withheld: {recovery}"
        );
        // Lines the room history already shows are not printed twice.
        assert_eq!(recovery.matches("filler 19").count(), 1);
        // A fresh session withholds nothing, so recovery adds no extra block.
        let fresh = super::super::context::build_delivery(
            &state,
            "claude",
            "suzy",
            "do it",
            None,
            "m19",
            super::super::context::Framing::Addressed,
        );
        assert!(fresh.withheld.is_empty());
        assert!(
            !super::super::context::recovery_context(&state, "claude", &fresh.withheld)
                .contains("Earlier lines you were assumed to remember")
        );
    }

    /// The extra blocks reach past the contiguous cursor. Whatever they showed
    /// is carried forward so the next turn's page does not walk back over it.
    #[test]
    fn lines_shown_ahead_of_the_cursor_are_not_delivered_a_second_time() {
        let (state, _) = test_state("local", "suzy");
        for i in 0..30 {
            say(
                &state,
                &format!("m{i}"),
                "human",
                "sender",
                &format!("message {i}"),
            );
        }
        let first = super::super::context::build_delivery(
            &state,
            "claude",
            "suzy",
            "do it",
            Some(&super::super::memory::Record {
                session_id: "session".into(),
                last_seen_message: "m0".into(),
                ..Default::default()
            }),
            "m29",
            super::super::context::Framing::Addressed,
        );
        assert_eq!(first.cursor.as_deref(), Some("m12"));
        // The tail block ran ahead of the cursor and showed these already.
        assert!(first.prompt.contains("message 20"));
        assert!(first.delivered.iter().any(|id| id == "m20"));

        let second = super::super::context::build_delivery(
            &state,
            "claude",
            "suzy",
            "do it again",
            Some(&super::super::memory::Record {
                session_id: "session".into(),
                last_seen_message: first.cursor.clone().unwrap(),
                delivered: first.delivered.clone(),
            }),
            "m29",
            super::super::context::Framing::Addressed,
        );
        assert!(
            !second.prompt.contains("message 20"),
            "already shown on the previous turn: {}",
            second.prompt
        );
        // Lines in the gap the tail block never reached are still delivered.
        assert!(second.prompt.contains("message 13"));
        // The memo only tracks what is still ahead of the cursor.
        assert!(!second.delivered.iter().any(|id| id == "m5"));
    }

    /// A thread ancestor that one of the room blocks already printed must not
    /// be printed again under its own heading.
    #[test]
    fn a_thread_ancestor_already_in_the_room_block_is_not_repeated() {
        let (state, _) = test_state("local", "suzy");
        for i in 0..30 {
            say(
                &state,
                &format!("m{i}"),
                "human",
                "sender",
                &format!("message {i}"),
            );
        }
        let mut trigger = super::super::inbox::tests::request("m29", "claude").message;
        trigger.reply_to = Some("m25".into());
        {
            use yrs::Array;
            let mut txn = state.control.transact_mut();
            let chat = txn.get_or_insert_array(crate::control::CHAT_KEY);
            chat.remove(&mut txn, 29);
            chat.push_back(
                &mut txn,
                Any::String(serde_json::to_string(&trigger).unwrap().into()),
            );
        }
        let delivered = super::super::context::build_delivery(
            &state,
            "claude",
            "suzy",
            "do it",
            Some(&super::super::memory::Record {
                session_id: "session".into(),
                last_seen_message: "m0".into(),
                ..Default::default()
            }),
            "m29",
            super::super::context::Framing::Addressed,
        );
        assert_eq!(
            delivered.prompt.matches("message 25").count(),
            1,
            "m25 is both a tail line and the thread ancestor: {}",
            delivered.prompt
        );
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
        assert!(tokio::time::timeout(LIVENESS, worker)
            .await
            .unwrap()
            .unwrap()
            .is_err());
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
        // Waiting on a freshly spawned task to publish the inbox.
        let inbox = tokio::time::timeout(LIVENESS, wait).await.unwrap();
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
        tokio::time::timeout(LIVENESS, recovered).await.unwrap();
        cancel.cancel();
    }
    /// The daemon loads the ledger at startup, before any message a test posts
    /// exists, so the first-run seed (which decides the whole transcript as
    /// pre-activation) stays out of the way.
    fn ledger_at_startup(dir: &std::path::Path) -> super::super::ledger::AmbientLedger {
        super::super::ledger::AmbientLedger::load(dir, &[], 0)
    }

    /// Post a message to the room and let the drain decide about it.
    fn offer_room(
        state: &AppState,
        handled: &super::super::handled::HandledMentions,
        inbox: &super::super::inbox::Inbox,
        ledger: &super::super::ledger::AmbientLedger,
        wake: &tokio::sync::Notify,
        cfg: &AgentConfig,
        message: &crate::control::ChatMessage,
    ) {
        add_chat(state, message);
        drain_ambient(state, handled, inbox, ledger, wake, cfg);
    }

    /// A human message that names nobody — the population ambient exists for.
    fn room_message(id: &str, text: &str) -> crate::control::ChatMessage {
        use super::super::inbox::tests::request;
        let mut message = request(id, "claude").message;
        message.text = text.into();
        message.mentions.clear();
        message.author = crate::control::Author::Human;
        message.ts = chrono::Utc::now().timestamp();
        message
    }

    #[test]
    fn a_message_too_short_to_answer_says_so() {
        use super::super::inbox::Inbox;
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let wake = tokio::sync::Notify::new();
        let ledger = ledger_at_startup(dir.path());
        let cfg = inbox_config();
        offer_room(
            &state,
            &handled,
            &inbox,
            &ledger,
            &wake,
            &cfg,
            &room_message("short", "anyone?"),
        );
        let skips = state.admission_log.recent(10);
        assert_eq!(skips.len(), 1);
        assert_eq!(skips[0].reason, "too short to be worth a turn");
        assert_eq!(skips[0].agent, None, "nobody in particular was skipped");
    }

    #[test]
    fn an_agents_own_reply_is_not_reported_as_a_missed_turn() {
        // The rule that makes ambient terminate fires on every agent post. It
        // is not a diagnostic, and reporting it would evict everything useful.
        use super::super::inbox::Inbox;
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let wake = tokio::sync::Notify::new();
        let ledger = ledger_at_startup(dir.path());
        let cfg = inbox_config();
        let mut message = room_message("reply", "I had a thought about the retry path");
        message.author = crate::control::Author::Agent;
        offer_room(&state, &handled, &inbox, &ledger, &wake, &cfg, &message);
        assert!(state.admission_log.recent(10).is_empty());
    }

    #[test]
    fn a_room_nobody_listens_to_reports_nothing_per_message() {
        // "No ambient agents" is configuration, surfaced by `readiness`. Writing
        // it once per message would be the only thing the buffer ever held.
        use super::super::inbox::Inbox;
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let wake = tokio::sync::Notify::new();
        let ledger = ledger_at_startup(dir.path());
        let mut cfg = inbox_config();
        cfg.ambient.clear();
        offer_room(
            &state,
            &handled,
            &inbox,
            &ledger,
            &wake,
            &cfg,
            &room_message("quiet", "Could someone explain how synchronization works?"),
        );
        assert!(state.admission_log.recent(10).is_empty());
    }

    #[test]
    fn a_listener_that_just_spoke_is_reported_rather_than_silently_dropped() {
        use super::super::inbox::Inbox;
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let wake = tokio::sync::Notify::new();
        let ledger = ledger_at_startup(dir.path());
        let cfg = inbox_config();
        let message = room_message("quiet-period", "Could someone explain the retry path?");
        let mut spoke = message.clone();
        spoke.id = "earlier".into();
        spoke.author = crate::control::Author::Agent;
        spoke.agent_id = "claude".into();
        add_chat(&state, &spoke);
        offer_room(&state, &handled, &inbox, &ledger, &wake, &cfg, &message);
        let skips = state.admission_log.recent(10);
        assert_eq!(skips.len(), 1);
        assert_eq!(
            skips[0].reason,
            "the only agent reading this room had just spoken"
        );
    }

    #[test]
    fn pull_mode_and_a_missing_agent_are_reported_only_when_meant_for_this_device() {
        use super::super::inbox::Inbox;
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let wake = tokio::sync::Notify::new();
        let message = room_message("addressed", "please take a look at the retry path");

        let mut pull = inbox_config();
        pull.reaction = Reaction::Pull;
        let req = |addressed_here| DispatchRequest {
            agent: "claude",
            mention_key: "claude",
            task: message.text.clone(),
            message: &message,
            relay: None,
            implicit: false,
            ambient: false,
            co_listeners: Vec::new(),
            addressed_here,
        };
        dispatch(&state, &handled, &inbox, &wake, &pull, req(true));
        let skips = state.admission_log.recent(10);
        assert_eq!(skips.len(), 1);
        assert!(skips[0].reason.contains("pull"));
        assert_eq!(skips[0].agent.as_deref(), Some("claude"));

        // A bare @claude fans out to every device. A machine that does not
        // configure it is not at fault, and must not fill the buffer saying so.
        let mut unknown = inbox_config();
        unknown.agents.remove("claude");
        dispatch(&state, &handled, &inbox, &wake, &unknown, req(false));
        assert_eq!(state.admission_log.recent(10).len(), 1, "unchanged");
        dispatch(&state, &handled, &inbox, &wake, &unknown, req(true));
        assert_eq!(
            state.admission_log.recent(10)[0].reason,
            "no agent by this name is configured on this device"
        );
    }

    #[test]
    fn a_turn_that_runs_after_all_clears_the_reason_it_did_not() {
        use super::super::inbox::Inbox;
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let wake = tokio::sync::Notify::new();
        let ledger = ledger_at_startup(dir.path());
        let cfg = inbox_config();
        let message = room_message("second-chance", "Could someone explain the retry path?");
        state
            .admission_log
            .record(&message.id, None, "too short to be worth a turn", 1);
        offer_room(&state, &handled, &inbox, &ledger, &wake, &cfg, &message);
        assert_eq!(inbox.entries().len(), 1, "it was offered a turn");
        assert!(
            state.admission_log.recent(10).is_empty(),
            "the stale reason is withdrawn"
        );
    }

    #[test]
    fn a_failed_turn_says_so_in_the_room_once() {
        use super::super::inbox::tests::request;
        let (state, _dir) = test_state("local", "suzy");
        let asked = room_message("asked", "how does the retry path work?");
        add_chat(&state, &asked);
        let mut failed = request("asked", "claude");
        failed.message = asked.clone();

        announce_failure(&state, &failed, "the adapter exited before starting");
        let posted: Vec<_> = state
            .transcript()
            .into_iter()
            .filter(|m| m.author == crate::control::Author::System)
            .collect();
        assert_eq!(posted.len(), 1);
        assert_eq!(posted[0].agent_id, "system", "renders as a system event");
        assert_eq!(posted[0].reply_to.as_deref(), Some("asked"));
        assert!(posted[0].text.starts_with("claude could not answer"));
        assert!(posted[0]
            .text
            .contains("the adapter exited before starting"));
        assert!(posted[0].relay.is_none(), "a system line roots no cascade");

        // A broken adapter in a busy room must not narrate every message.
        let mut again = failed.clone();
        again.message = room_message("asked-again", "seriously though, the retry path?");
        add_chat(&state, &again.message);
        announce_failure(&state, &again, "the adapter exited before starting");
        assert_eq!(
            state
                .transcript()
                .iter()
                .filter(|m| m.author == crate::control::Author::System)
                .count(),
            1,
            "throttled to one line per agent per window"
        );
    }

    #[test]
    fn a_failure_nobody_is_waiting_on_stays_quiet() {
        // Another agent already answered, so the room is not short of a reply
        // and a system line would only be noise.
        use super::super::inbox::tests::request;
        let (state, _dir) = test_state("local", "suzy");
        let asked = room_message("asked", "how does the retry path work?");
        add_chat(&state, &asked);
        let mut answer = room_message("answer", "it retries on the reconcile tick");
        answer.author = crate::control::Author::Agent;
        answer.agent_id = "codex".into();
        answer.reply_to = Some("asked".into());
        add_chat(&state, &answer);

        let mut failed = request("asked", "claude");
        failed.message = asked;
        announce_failure(&state, &failed, "the adapter exited before starting");
        assert!(!state
            .transcript()
            .iter()
            .any(|m| m.author == crate::control::Author::System));
    }

    #[test]
    fn a_listener_that_was_beaten_to_it_does_not_run() {
        // The turn sat behind a device permit while another agent answered.
        // Nobody asked this one for anything, so a second take is worse than
        // silence — and launch is the last moment the queue can tell.
        use super::super::inbox::tests::request;
        let (state, _dir) = test_state("local", "suzy");
        let cfg = inbox_config();
        let asked = room_message("asked", "how does the retry path work?");
        add_chat(&state, &asked);
        let mut req = request("asked", "claude");
        req.message = asked.clone();
        req.ambient = true;
        assert_eq!(
            launch_rejection(&state, &req, &cfg).unwrap(),
            None,
            "nothing has been said yet"
        );

        let mut answer = room_message("answer", "it retries on the reconcile tick");
        answer.author = crate::control::Author::Agent;
        answer.agent_id = "codex".into();
        answer.reply_to = Some("asked".into());
        add_chat(&state, &answer);
        assert_eq!(
            launch_rejection(&state, &req, &cfg).unwrap(),
            Some("another agent answered first")
        );

        // An addressed turn is not discharged by somebody else replying: you
        // named this agent, and you are still owed its answer.
        let mut addressed = req.clone();
        addressed.ambient = false;
        assert_eq!(launch_rejection(&state, &addressed, &cfg).unwrap(), None);
    }

    #[test]
    fn every_selected_listener_learns_about_the_others() {
        use super::super::inbox::Inbox;
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let wake = tokio::sync::Notify::new();
        let ledger = ledger_at_startup(dir.path());
        let mut cfg = inbox_config();
        cfg.ambient = vec!["claude".into(), "codex".into()];
        cfg.ambient_responders = 2;
        let message = room_message("both", "Could someone explain how synchronization works?");
        offer_room(&state, &handled, &inbox, &ledger, &wake, &cfg, &message);
        let entries = inbox.entries();
        assert_eq!(entries.len(), 2);
        for entry in &entries {
            // The whole selection, self included; the prompt filters self out.
            assert_eq!(
                entry.request.co_listeners,
                vec!["claude".to_string(), "codex".to_string()],
                "{} was not told who else is answering",
                entry.request.agent
            );
        }
    }

    #[test]
    fn an_addressed_turn_records_no_listeners() {
        use super::super::inbox::Inbox;
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let ledger = ledger_at_startup(dir.path());
        let wake = tokio::sync::Notify::new();
        let cfg = inbox_config();
        let mut message = room_message("named", "@claude please look at the retry path");
        message.mentions = vec!["claude".into()];
        admit_message(
            &state, &handled, &inbox, &ledger, &wake, &cfg, &message, true,
        );
        let entries = inbox.entries();
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].request.ambient);
        assert!(entries[0].request.co_listeners.is_empty());
    }

    #[test]
    fn an_explicit_reply_to_an_agent_does_not_also_wake_the_room() {
        // Deciding ambient in the drain instead of on the admission path left
        // the addressed-routing check behind. A reply aimed at one agent would
        // then reach it as a follow-up *and* be offered to the room, so the
        // question got answered twice.
        use super::super::inbox::Inbox;
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let wake = tokio::sync::Notify::new();
        let ledger = ledger_at_startup(dir.path());
        let cfg = inbox_config();

        let mut answer = room_message("answer", "here is how the retry path works");
        answer.author = crate::control::Author::Agent;
        answer.agent_id = "claude".into();
        answer.relay = Some(super::super::relay::extend(
            &super::super::relay::mint("root", "local"),
            "claude",
        ));
        // Outside the quiet period, so nothing else can explain the silence.
        answer.ts = chrono::Utc::now().timestamp() - 300;
        add_chat(&state, &answer);

        let mut followup = room_message("followup", "could you expand on the backoff part?");
        followup.reply_to = Some("answer".into());
        add_chat(&state, &followup);
        drain_ambient(&state, &handled, &inbox, &ledger, &wake, &cfg);
        assert!(
            inbox.entries().is_empty(),
            "the reply belongs to the agent it points at"
        );
        assert_eq!(
            ledger.decision("followup"),
            Some(super::super::ledger::Decision::Skipped(
                "part of an exchange with an agent".into()
            ))
        );
    }

    #[test]
    fn a_reply_whose_parent_has_not_synced_yet_waits_rather_than_guessing() {
        // It may turn out to be a reply to an agent. Deciding now would be a
        // guess; leaving it undecided costs one rescan and self-corrects.
        use super::super::inbox::Inbox;
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let wake = tokio::sync::Notify::new();
        let ledger = ledger_at_startup(dir.path());
        let cfg = inbox_config();

        let mut orphan = room_message("orphan", "could you expand on the backoff part?");
        orphan.reply_to = Some("not-synced-yet".into());
        add_chat(&state, &orphan);
        drain_ambient(&state, &handled, &inbox, &ledger, &wake, &cfg);
        assert!(inbox.entries().is_empty());
        assert!(
            !ledger.decided("orphan"),
            "undecided, so a later drain can look again"
        );

        // The parent arrives, and turns out to have been a person's.
        let mut parent = room_message("not-synced-yet", "what about the backoff?");
        parent.ts = chrono::Utc::now().timestamp() - 300;
        add_chat(&state, &parent);
        drain_ambient(&state, &handled, &inbox, &ledger, &wake, &cfg);
        assert!(ledger.decided("orphan"), "now it can be decided");
    }

    #[test]
    fn a_peer_whose_clock_runs_slow_can_still_mention_an_agent() {
        // The severe half of the clock defect, and the reason §2.4 exists. The
        // admission gate compared the *author's* `ts` against this device's
        // `activated_at`, so a peer whose clock sat behind that moment had its
        // explicit @mentions dropped here — silently, and for as long as the
        // Circle existed. Not ambient: addressed work.
        use super::super::inbox::{tests::request, Inbox};
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), chrono::Utc::now().timestamp()).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let ledger = ledger_at_startup(dir.path());
        let wake = tokio::sync::Notify::new();
        let cfg = inbox_config();

        let mut message = request("slow-peer", "claude").message;
        message.ts = chrono::Utc::now().timestamp() - 3600;
        message.mentions = vec!["claude".into()];
        add_chat(&state, &message);
        admit_message(
            &state, &handled, &inbox, &ledger, &wake, &cfg, &message, true,
        );
        let entries = inbox.entries();
        assert_eq!(entries.len(), 1, "an hour behind is not a reason to ignore");
        assert_eq!(entries[0].request.agent, "claude");
    }

    #[test]
    fn history_present_at_activation_never_fires_however_the_clocks_read() {
        // The property the timestamp was there for, and which must survive its
        // removal: switching agents on in a Circle with history must not fire
        // every @mention ever written in it.
        use super::super::inbox::{tests::request, Inbox};
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let wake = tokio::sync::Notify::new();
        let cfg = inbox_config();
        let mut history = Vec::new();
        for i in 0..5 {
            let mut m = request(&format!("past{i}"), "claude").message;
            m.mentions = vec!["claude".into()];
            // Timestamps all over the place, including the future.
            m.ts = chrono::Utc::now().timestamp() + (i as i64 - 2) * 3600;
            add_chat(&state, &m);
            history.push(m.id.clone());
        }
        let ledger = super::super::ledger::AmbientLedger::load(dir.path(), &history, 100);
        for id in &history {
            let message = state
                .transcript()
                .into_iter()
                .find(|m| &m.id == id)
                .unwrap();
            admit_message(
                &state, &handled, &inbox, &ledger, &wake, &cfg, &message, true,
            );
        }
        assert!(
            inbox.entries().is_empty(),
            "history present at activation stays history"
        );

        // But the next thing anyone says is new, whatever its timestamp.
        let mut fresh = request("after", "claude").message;
        fresh.mentions = vec!["claude".into()];
        fresh.ts = 1;
        add_chat(&state, &fresh);
        admit_message(&state, &handled, &inbox, &ledger, &wake, &cfg, &fresh, true);
        assert_eq!(inbox.entries().len(), 1);
        assert_eq!(inbox.entries()[0].request.message.id, "after");
    }

    #[test]
    fn a_room_with_no_listener_still_answers_a_mention() {
        // `no-listener` and `pre-activation` had to be separate decisions:
        // recording "nobody reads the room" as the activation boundary would
        // have made a Circle without an ambient agent ignore @mentions too.
        use super::super::inbox::Inbox;
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let ledger = ledger_at_startup(dir.path());
        let wake = tokio::sync::Notify::new();
        let mut cfg = inbox_config();
        cfg.ambient.clear();

        let mut message = room_message("quiet", "how does the retry path work here?");
        add_chat(&state, &message);
        drain_ambient(&state, &handled, &inbox, &ledger, &wake, &cfg);
        assert_eq!(
            ledger.decision("quiet"),
            Some(super::super::ledger::Decision::NoListener)
        );
        assert!(inbox.entries().is_empty());

        message.mentions = vec!["claude".into()];
        admit_message(
            &state, &handled, &inbox, &ledger, &wake, &cfg, &message, true,
        );
        assert_eq!(inbox.entries().len(), 1, "an @mention is still addressed");
    }

    #[test]
    fn a_message_authored_an_hour_ago_is_still_read() {
        // The defect this phase exists to fix. `ts` is the *author's* clock, so
        // a peer running a minute slow, or one syncing after a disconnection,
        // posted messages that were born stale and could never be read here.
        use super::super::inbox::Inbox;
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let wake = tokio::sync::Notify::new();
        let ledger = ledger_at_startup(dir.path());
        let cfg = inbox_config();
        let mut message = room_message("slow-clock", "how does the retry path work here?");
        message.ts = chrono::Utc::now().timestamp() - 3600;
        offer_room(&state, &handled, &inbox, &ledger, &wake, &cfg, &message);
        assert_eq!(inbox.entries().len(), 1, "an hour late is still a message");
        assert_eq!(inbox.entries()[0].request.agent, "claude");
    }

    #[test]
    fn a_clock_running_fast_does_not_buy_a_second_turn() {
        // The other direction of skew: a message from the future must be read
        // exactly once, not re-read on every drain until its timestamp passes.
        use super::super::inbox::Inbox;
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let wake = tokio::sync::Notify::new();
        let ledger = ledger_at_startup(dir.path());
        let cfg = inbox_config();
        let mut message = room_message("fast-clock", "how does the retry path work here?");
        message.ts = chrono::Utc::now().timestamp() + 3600;
        offer_room(&state, &handled, &inbox, &ledger, &wake, &cfg, &message);
        drain_ambient(&state, &handled, &inbox, &ledger, &wake, &cfg);
        drain_ambient(&state, &handled, &inbox, &ledger, &wake, &cfg);
        assert_eq!(inbox.entries().len(), 1);
    }

    #[test]
    fn a_backlog_collapses_to_its_tail_instead_of_being_discarded() {
        // Both old behaviours were wrong in opposite directions: a live message
        // 31 seconds old got nothing, and a 500-message backfill also got
        // nothing — affordable, but by accident.
        use super::super::inbox::Inbox;
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let wake = tokio::sync::Notify::new();
        let ledger = ledger_at_startup(dir.path());
        let cfg = inbox_config();
        for i in 0..500 {
            add_chat(
                &state,
                &room_message(&format!("m{i:03}"), "how does the retry path work here?"),
            );
        }
        let decided = drain_ambient(&state, &handled, &inbox, &ledger, &wake, &cfg);
        assert_eq!(decided, 500, "every message is decided, once");
        assert_eq!(ledger.len(), 500);
        let entries = inbox.entries();
        assert_eq!(entries.len(), 1, "exactly the configured tail runs");
        assert_eq!(
            entries[0].request.message.id, "m499",
            "the tail is the newest by CRDT order, not by anyone's clock"
        );
        assert_eq!(
            ledger.decision("m499"),
            Some(super::super::ledger::Decision::Offered)
        );
        assert_eq!(
            ledger.decision("m000"),
            Some(super::super::ledger::Decision::Backlog),
            "everything behind the tail is decided, not left to be re-decided"
        );
        // And draining again decides nothing, however many times it runs.
        assert_eq!(
            drain_ambient(&state, &handled, &inbox, &ledger, &wake, &cfg),
            0
        );
        assert_eq!(inbox.entries().len(), 1);
    }

    #[test]
    fn a_backlog_spends_its_turn_on_a_question_not_on_ok_thanks() {
        // The cheap gates run before the tail is chosen, so the newest message
        // being chatter does not waste the one turn a backlog gets.
        use super::super::inbox::Inbox;
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let wake = tokio::sync::Notify::new();
        let ledger = ledger_at_startup(dir.path());
        let cfg = inbox_config();
        add_chat(
            &state,
            &room_message("q", "how does the retry path work here?"),
        );
        add_chat(&state, &room_message("chatter", "ok"));
        add_chat(&state, &room_message("more", "thanks!"));
        drain_ambient(&state, &handled, &inbox, &ledger, &wake, &cfg);
        let entries = inbox.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].request.message.id, "q");
    }

    #[test]
    fn decisions_survive_a_restart() {
        use super::super::inbox::Inbox;
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let wake = tokio::sync::Notify::new();
        let cfg = inbox_config();
        let message = room_message("once", "how does the retry path work here?");
        {
            let ledger = ledger_at_startup(dir.path());
            offer_room(&state, &handled, &inbox, &ledger, &wake, &cfg, &message);
            assert_eq!(inbox.entries().len(), 1);
        }
        // A new daemon reloads the ledger from disk, this time with the
        // transcript already populated — which must not re-decide anything.
        let ids: Vec<String> = state.transcript().into_iter().map(|m| m.id).collect();
        let reloaded = super::super::ledger::AmbientLedger::load(dir.path(), &ids, 200);
        assert_eq!(
            drain_ambient(&state, &handled, &inbox, &reloaded, &wake, &cfg),
            0,
            "a reconnect replaying the transcript must not re-offer it"
        );
        assert_eq!(inbox.entries().len(), 1);
    }

    #[test]
    fn a_room_with_no_listeners_decides_without_spending_anything() {
        // Turning a listener on later must not retroactively answer everything
        // said while none was configured.
        use super::super::inbox::Inbox;
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let wake = tokio::sync::Notify::new();
        let ledger = ledger_at_startup(dir.path());
        let mut cfg = inbox_config();
        cfg.ambient.clear();
        for i in 0..3 {
            add_chat(
                &state,
                &room_message(&format!("quiet{i}"), "how does the retry path work here?"),
            );
        }
        assert_eq!(
            drain_ambient(&state, &handled, &inbox, &ledger, &wake, &cfg),
            3
        );
        assert!(inbox.entries().is_empty());

        cfg.ambient = vec!["claude".into()];
        assert_eq!(
            drain_ambient(&state, &handled, &inbox, &ledger, &wake, &cfg),
            0
        );
        assert!(
            inbox.entries().is_empty(),
            "switching a listener on is not retroactive"
        );
    }

    #[test]
    fn a_listener_named_in_a_different_case_still_reads_the_room() {
        // `is_ambient` compares case-insensitively while the configured-agent
        // filter did not, so `ambient = ["Claude"]` against `[agents.claude]`
        // was silently inert (§6).
        use super::super::inbox::Inbox;
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let wake = tokio::sync::Notify::new();
        let ledger = ledger_at_startup(dir.path());
        let mut cfg = inbox_config();
        cfg.ambient = vec!["Claude".into()];
        offer_room(
            &state,
            &handled,
            &inbox,
            &ledger,
            &wake,
            &cfg,
            &room_message("cased", "how does the retry path work here?"),
        );
        assert_eq!(inbox.entries().len(), 1);
    }

    /// Drive `run_id` to a terminal state the way a real run would.
    fn land(inbox: &super::super::inbox::Inbox, run_id: &str, end: super::super::inbox::Status) {
        use super::super::inbox::Status;
        inbox
            .transition(run_id, Status::Pending, Status::Running, None, 101)
            .unwrap();
        inbox
            .transition(run_id, Status::Running, end, Some("boom".into()), 102)
            .unwrap();
    }

    #[test]
    fn a_failed_turn_hands_the_message_to_the_next_listener() {
        use super::super::inbox::{Inbox, Status};
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let wake = tokio::sync::Notify::new();
        let ledger = ledger_at_startup(dir.path());
        let mut cfg = inbox_config();
        cfg.ambient = vec!["claude".into(), "codex".into()];
        let message = room_message("dropped", "how does the retry path work here?");
        offer_room(&state, &handled, &inbox, &ledger, &wake, &cfg, &message);
        let first = inbox.entries();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].request.agent, "claude");

        // Still running: the room is not waiting on anyone else yet.
        drain_ambient(&state, &handled, &inbox, &ledger, &wake, &cfg);
        assert_eq!(inbox.entries().len(), 1);

        land(&inbox, &first[0].run_id, Status::Failed);
        drain_ambient(&state, &handled, &inbox, &ledger, &wake, &cfg);
        assert!(
            !state
                .transcript()
                .iter()
                .any(|m| m.author == crate::control::Author::System),
            "one failure is not the end of the message, so the room is not told"
        );
        let after = inbox.entries();
        assert_eq!(after.len(), 2, "the next listener gets a go");
        assert_eq!(after[1].request.agent, "codex");
        assert_eq!(after[1].request.message.id, "dropped");
    }

    #[test]
    fn a_pass_settles_the_message_and_a_cancel_does_too() {
        // PASS is a considered answer, and a cancellation is a decision this
        // device made — "another agent answered first", say. Reopening either
        // would undo it.
        use super::super::inbox::{Inbox, Status};
        for (end, label) in [(Status::Completed, "pass"), (Status::Cancelled, "cancel")] {
            let (state, dir) = test_state("local", "suzy");
            let inbox = Inbox::open(dir.path(), 100).unwrap();
            let handled = super::super::handled::HandledMentions::load(dir.path());
            let wake = tokio::sync::Notify::new();
            let ledger = ledger_at_startup(dir.path());
            let mut cfg = inbox_config();
            cfg.ambient = vec!["claude".into(), "codex".into()];
            let message = room_message("settled", "how does the retry path work here?");
            offer_room(&state, &handled, &inbox, &ledger, &wake, &cfg, &message);
            let run = inbox.entries()[0].run_id.clone();
            match end {
                Status::Cancelled => {
                    inbox
                        .transition(&run, Status::Pending, end, None, 102)
                        .unwrap();
                }
                _ => land(&inbox, &run, end),
            }
            drain_ambient(&state, &handled, &inbox, &ledger, &wake, &cfg);
            assert_eq!(inbox.entries().len(), 1, "{label} must settle the message");
        }
    }

    #[test]
    fn a_message_that_breaks_every_adapter_costs_a_bounded_number_of_turns() {
        use super::super::inbox::{Inbox, Status};
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let wake = tokio::sync::Notify::new();
        let ledger = ledger_at_startup(dir.path());
        let mut cfg = inbox_config();
        cfg.ambient = vec!["claude".into(), "codex".into()];
        cfg.ambient_max_attempts = 2;
        let message = room_message("poison", "how does the retry path work here?");
        offer_room(&state, &handled, &inbox, &ledger, &wake, &cfg, &message);
        for _ in 0..5 {
            for entry in inbox
                .entries()
                .into_iter()
                .filter(|e| e.status == Status::Pending)
            {
                land(&inbox, &entry.run_id, Status::Failed);
            }
            drain_ambient(&state, &handled, &inbox, &ledger, &wake, &cfg);
        }
        assert_eq!(inbox.entries().len(), 2, "one turn per attempt, then stop");
        // And the room is told once, rather than left looking ignored.
        let announced: Vec<_> = state
            .transcript()
            .into_iter()
            .filter(|m| m.author == crate::control::Author::System)
            .collect();
        assert_eq!(announced.len(), 1);
        assert!(announced[0].text.starts_with("No agent could answer"));
        assert_eq!(announced[0].reply_to.as_deref(), Some("poison"));
        assert_eq!(
            ledger.decision("poison"),
            Some(super::super::ledger::Decision::Skipped(
                "every attempt failed".into()
            ))
        );
    }

    #[test]
    fn two_listeners_plus_one_restart_is_not_two_failed_attempts() {
        // Observed in a real Circle: `ambient_responders = 2` admits two
        // entries in the opening round, so counting entries left `2 >= 2` after
        // a single restart. The message settled as "every attempt failed" and
        // the room was told nobody could answer — when nothing had run at all.
        use super::super::inbox::{Inbox, Status};
        let (state, dir) = test_state("local", "suzy");
        let mut inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let wake = tokio::sync::Notify::new();
        let ledger = ledger_at_startup(dir.path());
        let mut cfg = inbox_config();
        cfg.ambient = vec!["claude".into(), "codex".into()];
        cfg.ambient_responders = 2;
        cfg.ambient_max_attempts = 2;

        let message = room_message("both", "how does the retry path work here?");
        offer_room(&state, &handled, &inbox, &ledger, &wake, &cfg, &message);
        assert_eq!(inbox.entries().len(), 2, "both listeners offered");

        // The restart everyone hits while iterating on a build.
        drop(inbox);
        inbox = Inbox::open(dir.path(), 101).unwrap();
        assert!(inbox.entries().iter().all(|e| e.status == Status::Expired));

        drain_ambient(&state, &handled, &inbox, &ledger, &wake, &cfg);
        assert!(
            inbox.entries().iter().any(|e| e.status == Status::Pending),
            "a turn that never ran is put back, not counted against the budget"
        );
        assert!(
            !state
                .transcript()
                .iter()
                .any(|m| m.author == crate::control::Author::System),
            "and the room is not told nobody could answer"
        );
    }

    #[test]
    fn a_machine_that_keeps_restarting_stops_requeueing_eventually() {
        // The bound the entry count was there for, kept — just applied to the
        // pathology rather than to the opening round.
        use super::super::inbox::{Inbox, Status};
        let (state, dir) = test_state("local", "suzy");
        let mut inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let wake = tokio::sync::Notify::new();
        let ledger = ledger_at_startup(dir.path());
        let cfg = inbox_config();
        offer_room(
            &state,
            &handled,
            &inbox,
            &ledger,
            &wake,
            &cfg,
            &room_message("doomed", "how does the retry path work here?"),
        );
        let mut restarts = 0;
        for _ in 0..10 {
            drop(inbox);
            inbox = Inbox::open(dir.path(), 101).unwrap();
            restarts += 1;
            drain_ambient(&state, &handled, &inbox, &ledger, &wake, &cfg);
            if !inbox.entries().iter().any(|e| e.status == Status::Pending) {
                break;
            }
        }
        assert!(
            restarts <= EXPIRED_REQUEUE_LIMIT + 1,
            "gave up after {restarts} restarts"
        );
        assert_eq!(
            ledger.decision("doomed"),
            Some(super::super::ledger::Decision::Skipped(
                "this device kept restarting before it could run".into()
            ))
        );
    }

    #[test]
    fn a_turn_that_never_started_is_requeued_rather_than_blamed() {
        // A daemon restart expires whatever was queued. That agent did not try
        // and fail — it never ran — so telling the room "every agent tried"
        // would be a lie, and with one listener there is nobody to rotate to.
        use super::super::inbox::{Inbox, Status};
        let (state, dir) = test_state("local", "suzy");
        let mut inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let wake = tokio::sync::Notify::new();
        let ledger = ledger_at_startup(dir.path());
        let cfg = inbox_config();
        assert_eq!(cfg.ambient, vec!["claude".to_string()], "one listener");
        let message = room_message("expired", "how does the retry path work here?");
        offer_room(&state, &handled, &inbox, &ledger, &wake, &cfg, &message);

        drop(inbox);
        inbox = Inbox::open(dir.path(), 101).unwrap();
        assert_eq!(inbox.entries()[0].status, Status::Expired);
        drain_ambient(&state, &handled, &inbox, &ledger, &wake, &cfg);
        let entries = inbox.entries();
        assert_eq!(entries.len(), 2, "the same agent gets another go");
        assert_eq!(entries[1].request.agent, "claude");
        assert_eq!(entries[1].status, Status::Pending);
    }

    #[test]
    fn selected_listeners_get_one_queued_turn() {
        use super::super::inbox::{tests::request, Inbox};
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let wake = tokio::sync::Notify::new();
        let ledger = ledger_at_startup(dir.path());
        let mut cfg = inbox_config();
        cfg.ambient = vec!["claude".into(), "codex".into()];
        cfg.ambient_responders = 2;
        let mut message = request("all-listeners", "claude").message;
        message.text = "Could someone help explain how synchronization works?".into();
        message.mentions.clear();
        offer_room(&state, &handled, &inbox, &ledger, &wake, &cfg, &message);
        offer_room(&state, &handled, &inbox, &ledger, &wake, &cfg, &message);
        let entries = inbox.entries();
        assert_eq!(entries.len(), 2);
        assert!(entries
            .iter()
            .all(|e| e.request.ambient && e.status == super::super::inbox::Status::Pending));
        assert_eq!(
            entries
                .iter()
                .map(|e| e.request.agent.as_str())
                .collect::<Vec<_>>(),
            ["claude", "codex"]
        );
    }

    #[test]
    fn old_ambient_markers_import_as_real_agent_history() {
        use super::super::inbox::{tests::request, Inbox, Status};
        let (state, dir) = test_state("local", "suzy");
        let inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let message = request("old-observation", "claude").message;
        add_chat(&state, &message);
        handled.mark_new(&message.id, "~ambient:claude");
        reconcile_requests_with_config(
            &state,
            &handled,
            &inbox,
            &ledger_at_startup(dir.path()),
            &tokio::sync::Notify::new(),
            &inbox_config(),
        );
        let entries = inbox.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].request.agent, "claude");
        assert!(entries[0].request.ambient);
        assert_eq!(entries[0].status, Status::LegacySuppressed);
        assert!(inbox.retry(&entries[0].run_id, 20, 101).is_err());
    }
    #[test]
    fn listener_rotation_survives_restart_and_can_vary_the_count() {
        use super::super::inbox::{tests::request, Inbox};
        let (state, dir) = test_state("local", "suzy");
        let mut inbox = Inbox::open(dir.path(), 100).unwrap();
        let handled = super::super::handled::HandledMentions::load(dir.path());
        let wake = tokio::sync::Notify::new();
        let ledger = ledger_at_startup(dir.path());
        let mut cfg = inbox_config();
        cfg.ambient = vec!["claude".into(), "codex".into()];
        cfg.ambient_responders = 1;
        for i in 0..4 {
            let mut message = request(&format!("rotate{i}"), "claude").message;
            message.text = "Please explain how the synchronization strategy works".into();
            message.mentions.clear();
            offer_room(&state, &handled, &inbox, &ledger, &wake, &cfg, &message);
            let entries = inbox.entries();
            let entry = entries
                .iter()
                .find(|e| e.request.message.id == message.id)
                .expect("the new message was offered");
            assert_eq!(
                entry.request.agent,
                if i % 2 == 0 { "claude" } else { "codex" }
            );
            // Settle it before the restart. A turn left pending would expire on
            // reload, and an expired observation is now re-offered rather than
            // lost — correct, but not what this test is about.
            for (from, to) in [
                (
                    super::super::inbox::Status::Pending,
                    super::super::inbox::Status::Running,
                ),
                (
                    super::super::inbox::Status::Running,
                    super::super::inbox::Status::Completed,
                ),
            ] {
                inbox
                    .transition(&entry.run_id, from, to, None, 101)
                    .unwrap();
            }
            drop(inbox);
            inbox = Inbox::open(dir.path(), 101).unwrap();
        }
        cfg.ambient_responders = 2;
        cfg.ambient_rotate_count = true;
        let mut counts = Vec::new();
        for i in 4..14 {
            let mut message = request(&format!("rotate{i}"), "claude").message;
            message.text = "Please explain how the synchronization strategy works".into();
            message.mentions.clear();
            offer_room(&state, &handled, &inbox, &ledger, &wake, &cfg, &message);
            counts.push(
                inbox
                    .entries()
                    .iter()
                    .filter(|e| e.request.message.id == message.id)
                    .count(),
            );
        }
        assert!(
            counts.iter().all(|n| (1..=2).contains(n)),
            "within the configured limit: {counts:?}"
        );
        assert!(
            counts.contains(&1) && counts.contains(&2),
            "and it actually varies: {counts:?}"
        );
    }

    #[test]
    fn how_many_listeners_a_message_gets_does_not_depend_on_when_you_ask() {
        // The count used to come from a running tally of past offers, held in
        // an inbox that is trimmed and rebuilt across restarts — so the "cycle"
        // the setting promises was not reproducible, and the same message could
        // draw a different number of agents depending on when the daemon last
        // started (§6). It is derived from the message id now.
        let sample: Vec<u64> = ["m1", "m2", "m3", "a-longer-message-id"]
            .iter()
            .map(|id| fnv1a(id))
            .collect();
        assert_eq!(
            sample,
            ["m1", "m2", "m3", "a-longer-message-id"]
                .iter()
                .map(|id| fnv1a(id))
                .collect::<Vec<_>>(),
            "same id, same answer, every time"
        );
        assert!(
            sample.windows(2).any(|w| w[0] % 2 != w[1] % 2),
            "and different ids do not all land on the same count"
        );
    }
}
