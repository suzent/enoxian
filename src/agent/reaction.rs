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

    // Durable dedup: act on each (message, mention) at most once *ever*, not
    // just once per run. On reconnect, P2P sync replays the whole chat history
    // as fresh CRDT updates and the observer fires AgentMentioned for every
    // historical message; without a persisted guard, every past mention would
    // re-launch its agent on each restart. This survives restarts.
    let handled = super::handled::HandledMentions::load(&state.circle_dir);

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
                Ok(CircleEvent::AgentMentioned { agent_id, message }) => {
                    // Old message (replayed history) — skip cheaply.
                    if message.ts < cutoff {
                        continue;
                    }
                    // Never act on the same mention twice, across restarts.
                    if !handled.mark_new(&message.id, &agent_id) {
                        tracing::debug!("[agent] mention {}::{agent_id} already handled — skipping", message.id);
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

                    let sender = message.agent_id.clone();
                    let state = state.clone();
                    tokio::spawn(async move {
                        if let Err(e) = react(&state, &agent_id, &cmd, &task, &sender, initiator).await {
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
    sender: &str,
    initiator: Initiator,
) -> anyhow::Result<()> {
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
    let prompt = super::context::build_prompt(state, agent_id, sender, task, resume.is_some());

    let outcome = driver::launch(driver::LaunchRequest {
        agent_name: agent_id,
        cmd,
        task: &prompt,
        workspace: &state.workspace,
        base_snapshot: &base_snapshot,
        circle_id: &state.circle_id,
        initiator,
        resume: resume.as_deref(),
    })
    .await?;

    // Remember the ACP session so the next mention continues the conversation.
    if let Some(sid) = &outcome.acp_session_id {
        if let Err(e) = super::memory::save(&state.circle_dir, agent_id, sid) {
            tracing::warn!("[agent] failed to persist session for `{agent_id}`: {e}");
        }
    }

    tracing::info!(
        "[agent] `{agent_id}` finished: session={} {}",
        outcome.session_id, outcome.detail
    );

    // Post the agent's streamed reply back into the chat room so the mention
    // reads like a conversation. File changes still surface separately as a
    // proposal (via the ambient engine + pull protocol); this is the
    // conversational half.
    if let Some(reply) = outcome.reply.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        // Post under the agent's name WITHOUT firing mention triggers: an
        // agent's reply must never wake another agent, or two agents ping-pong
        // forever. (fire_mentions = false)
        let _ = crate::api::chat::post_message(state, agent_id.to_string(), reply.to_string(), false);
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
