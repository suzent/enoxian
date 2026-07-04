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
//! does not configure is ignored. Whether the proposal the run produces
//! auto-accepts is decided later by the acceptance policy, keyed on whether the
//! mention was posted locally or by a remote member.

use super::config::{AgentConfig, Reaction};
use super::driver::{self, Initiator};
use super::mention::Mention;
use crate::control::CircleEvent;
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
    tracing::info!("[agent] reaction loop started for circle {}", state.circle_id);

    // Dedup: the same mention can surface more than once (e.g. the local HTTP
    // post and the CRDT observer both emit AgentMentioned, or a message
    // re-delivers). Launching an agent is expensive and side-effectful, so act
    // at most once per (message id, target). Bounded FIFO to cap memory.
    let mut handled: std::collections::VecDeque<String> = std::collections::VecDeque::new();

    loop {
        tokio::select! {
            _ = token.cancelled() => break,
            evt = events.recv() => match evt {
                Ok(CircleEvent::AgentMentioned { agent_id, message }) => {
                    let dedup_key = format!("{}::{}", message.id, agent_id);
                    if handled.contains(&dedup_key) {
                        tracing::debug!("[agent] mention {dedup_key} already handled — skipping duplicate");
                        continue;
                    }
                    handled.push_back(dedup_key);
                    if handled.len() > 256 { handled.pop_front(); }

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
                            tracing::debug!(
                                "[agent] mention scoped to {owner}/{device}, not this device — skipping"
                            );
                            continue;
                        }
                    }

                    // Reload config per mention so edits to agents.toml take
                    // effect without a daemon restart — mentions are rare.
                    let cfg = AgentConfig::load();
                    if cfg.reaction != Reaction::Push {
                        tracing::debug!("[agent] pull policy — ignoring mention of `{agent}`");
                        continue;
                    }
                    let Some(cmd) = cfg.resolve(agent).cloned() else {
                        // Not one of this device's agents — nothing to do.
                        continue;
                    };

                    // The task is the message text with the leading @mention
                    // (in its full, possibly-scoped form) stripped, so the agent
                    // gets a clean instruction. Capture before shadowing.
                    let task = strip_mention(&message.text, &agent_id);
                    let agent_id = agent.to_string();

                    // Sender-origin sets the acceptance-policy posture: a mention
                    // posted from this very device is local; anything else is a
                    // remote member's request.
                    let initiator = if message.agent_id == state.agent_id {
                        Initiator::Local
                    } else {
                        Initiator::RemoteMember
                    };

                    let state = state.clone();
                    tokio::spawn(async move {
                        if let Err(e) = react(&state, &agent_id, &cmd, &task, initiator).await {
                            tracing::warn!("[agent] run of `{agent_id}` failed: {e:#}");
                        }
                    });
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

async fn react(
    state: &AppState,
    agent_id: &str,
    cmd: &super::config::AgentCommand,
    task: &str,
    initiator: Initiator,
) -> anyhow::Result<()> {
    // Anchor the change session on the engine's current baseline (S0) so the
    // agent's edits diff cleanly against it.
    let store = ProposalStore::open(&state.workspace)?;
    let base_snapshot = store.baseline_id().unwrap_or_default();

    let outcome = driver::launch(
        agent_id,
        cmd,
        task,
        &state.workspace,
        &base_snapshot,
        &state.circle_id,
        initiator,
    )
    .await?;

    tracing::info!(
        "[agent] `{agent_id}` finished: session={} {}",
        outcome.session_id, outcome.detail
    );

    // Post the agent's streamed reply back into the chat room so the mention
    // reads like a conversation. File changes still surface separately as a
    // proposal (via the ambient engine + pull protocol); this is the
    // conversational half.
    if let Some(reply) = outcome.reply.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        // Post under the agent's name. NOTE: if a reply itself contains an
        // @mention it will re-fire AgentMentioned — keep agent replies from
        // mentioning runnable agents to avoid trigger loops.
        let _ = crate::api::chat::post_message(state, agent_id.to_string(), reply.to_string());
    } else {
        tracing::debug!("[agent] `{agent_id}` produced no text reply to post");
    }
    Ok(())
}

/// Whether a mention scoped to `owner/device` addresses this device. Matches
/// this circle's owner and the local device label (case-insensitive, since
/// users type these by hand). If the local device has no label set, a device-
/// scoped mention can never match it — the user should set one to be
/// addressable (`enox identity set-label`).
fn targets_this_device(state: &AppState, owner: &str, device: &str) -> bool {
    if !state.owner.eq_ignore_ascii_case(owner) {
        return false;
    }
    let local_device = crate::identity::read_identity_display()
        .map(|(label, _)| label)
        .unwrap_or_default();
    !local_device.is_empty() && local_device.eq_ignore_ascii_case(device)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_leading_mention() {
        assert_eq!(strip_mention("@claude fix the docs", "claude"), "fix the docs");
        assert_eq!(strip_mention("  @claude  do it ", "claude"), "do it");
    }

    #[test]
    fn leaves_non_leading_mention() {
        assert_eq!(
            strip_mention("please ping @claude later", "claude"),
            "please ping @claude later"
        );
    }
}
