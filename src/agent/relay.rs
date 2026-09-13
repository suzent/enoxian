//! Delegation bounds: how an agent's reply may wake another agent, and why
//! that cannot run away.
//!
//! An agent reply used to fire no triggers at all (`fire_mentions = false`),
//! because a total ban was the only bound that needed no bookkeeping. This
//! module is the bookkeeping, so the ban can be narrowed to a budget. See
//! `docs/development/engagement.md` §3.
//!
//! Three bounds, because each alone has a shape it does not catch:
//!
//! - **Budget** — a cascade may cost at most `max_relay_turns` agent turns in
//!   total. This is the only bound independent of the cascade's *shape*.
//! - **Fan-out** — at most one agent-level mention in an agent reply is
//!   honoured. Without it a budget of N is a budget of N *levels*, which is
//!   exponential, not linear.
//! - **No self-trigger** — an agent never wakes itself. This kills the
//!   degenerate one-agent loop outright.
//!
//! Note what is deliberately *not* forbidden: `A -> B -> A`. Two agents
//! iterating on a problem is the thing delegation is for, so ping-pong is
//! bounded by the budget rather than made structurally impossible. That is the
//! trade the budget buys, and it is why the budget is large enough to be
//! useful and small enough to be survivable.

use crate::control::{ChatMessage, Relay};

/// Default ceiling on agent turns per cascade.
///
/// Sized for a real back-and-forth rather than a single hand-off: two agents
/// iterating burn a turn each per exchange, so a budget of 3 buys one and a
/// half exchanges. The cost of the ceiling being generous is bounded and
/// visible (the activity indicator shows the cascade, and cancelling an agent
/// cancels the rest of it); the cost of it being stingy is a feature that
/// stops mid-thought.
pub const DEFAULT_MAX_RELAY_TURNS: u8 = 20;

/// Hard ceiling on what any device will accept, whatever its config or a
/// peer's wire value says. A misconfigured `max_relay_turns = 250` should not
/// be able to hand one chat message a 250-turn bill.
pub const RELAY_TURNS_CEILING: u8 = 50;

/// Start a fresh cascade at a human message.
pub fn mint(root_message_id: &str, root_peer: &str) -> Relay {
    Relay {
        root: root_message_id.to_string(),
        root_peer: root_peer.to_string(),
        spent: 0,
        path: Vec::new(),
    }
}

/// The relay an agent's own reply carries: one more turn spent, and itself
/// appended to the branch.
pub fn extend(parent: &Relay, agent: &str) -> Relay {
    let mut path = parent.path.clone();
    path.push(agent.to_string());
    Relay {
        root: parent.root.clone(),
        root_peer: parent.root_peer.clone(),
        spent: parent.spent.saturating_add(1),
        path,
    }
}

/// The agent whose turn produced the message carrying this relay, if any.
/// `None` for a human message (empty path).
pub fn poster(relay: &Relay) -> Option<&str> {
    relay.path.last().map(String::as_str)
}

/// Is there budget left to wake *another* agent off a message carrying this
/// relay? `spent` counts the turns already taken, so the next turn is allowed
/// while it is strictly below the ceiling.
pub fn has_budget(relay: &Relay, max: u8) -> bool {
    relay.spent < max.min(RELAY_TURNS_CEILING)
}

/// Which mentions on an **agent reply** may fire a trigger: at most the first
/// that does not name the agent itself, and only while the budget holds.
///
/// Callers decide the human and system cases themselves — see
/// [`crate::api::chat::Trigger`]. They cannot be inferred from the relay
/// being absent: a system post and a message from a peer predating the field
/// both lack one, and they need opposite answers.
///
/// This is the *sender-side* filter. It is a courtesy, not the gate: the
/// device that would actually run the agent re-checks the budget against its
/// own configuration in [`crate::agent::reaction`].
pub fn triggerable_mentions(
    msg: &ChatMessage,
    max: u8,
    self_device: Option<(&str, &str)>,
) -> Vec<String> {
    let Some(relay) = &msg.relay else {
        // Predates the field: treat as a human post with no allowance to pass
        // on. Its own mentions still fire.
        return msg.mentions.clone();
    };
    let Some(posting_agent) = poster(relay) else {
        return msg.mentions.clone();
    };
    if !has_budget(relay, max) {
        tracing::debug!(
            "[relay] budget spent ({}/{max}) on cascade {} — {} mention(s) left inert",
            relay.spent,
            relay.root,
            msg.mentions.len()
        );
        return Vec::new();
    }
    msg.mentions
        .iter()
        .find(|m| {
            super::mention::Mention::parse(m).is_some_and(|target| target.agent_target().is_some())
                && !is_self_mention(m, posting_agent, self_device)
        })
        .cloned()
        .into_iter()
        .collect()
}

/// Would this mention wake the agent that posted it — the *same* agent on the
/// *same* machine?
///
/// A name match alone is not enough, and assuming it was is what broke
/// cross-device delegation: `claude` on one device mentioning
/// `@suzy/other-box/claude` is addressing a different agent that happens to
/// share a name, not itself. Treating that as a self-mention meant the hand-off
/// never fired, and nothing said why.
///
/// So a mention is self-addressed only when it names the same agent *and* is
/// scoped to this device. Anything else is left to the receiving side, which
/// can answer exactly — it knows whether it is the machine that posted the
/// message — and refusing here would lose the legitimate case of one device's
/// `claude` waking another's.
fn is_self_mention(mention: &str, posting_agent: &str, self_device: Option<(&str, &str)>) -> bool {
    let Some((name, scope)) = super::mention::Mention::parse(mention).and_then(|m| {
        m.agent_target().map(|(n, s)| {
            (
                n.to_string(),
                s.map(|(o, d)| (o.to_string(), d.to_string())),
            )
        })
    }) else {
        return false;
    };
    if name != posting_agent {
        return false;
    }
    match scope {
        // Bare `@claude` from `claude`: on this device it resolves to this very
        // agent, so it is self-addressed and skipped — which also lets the next
        // mention take the one honoured hand-off slot instead of wasting it.
        None => true,
        // Scoped to this very device, by the agent that lives here.
        Some((owner, device)) => match self_device {
            Some((self_owner, self_dev)) => {
                owner.eq_ignore_ascii_case(self_owner) && device.eq_ignore_ascii_case(self_dev)
            }
            // We do not know what this device is called yet. Let it through:
            // the receiving side compares peer ids, which is exact, and
            // refusing here would break delegation whenever the roster is
            // still syncing.
            None => false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The device these tests run "on".
    const HERE: Option<(&str, &str)> = Some(("suzy", "macbook-pro"));

    fn msg(mentions: &[&str], relay: Option<Relay>) -> ChatMessage {
        ChatMessage {
            thread_root: None,
            id: "m1".into(),
            agent_id: "claude".into(),
            text: String::new(),
            mentions: mentions.iter().map(|s| s.to_string()).collect(),
            ts: 0,
            peer_id: "p1".into(),
            attachments: Vec::new(),
            relay,
            author: crate::control::Author::Human,
            reply_to: None,
        }
    }

    #[test]
    fn human_post_fires_every_mention() {
        let m = msg(&["claude", "codex"], Some(mint("m0", "p1")));
        assert_eq!(triggerable_mentions(&m, 20, HERE), vec!["claude", "codex"]);
    }

    #[test]
    fn message_predating_the_field_still_fires() {
        let m = msg(&["claude"], None);
        assert_eq!(triggerable_mentions(&m, 20, HERE), vec!["claude"]);
    }

    #[test]
    fn agent_reply_fires_only_its_first_mention() {
        let r = extend(&mint("m0", "p1"), "claude");
        let m = msg(&["codex", "gemini"], Some(r));
        assert_eq!(triggerable_mentions(&m, 20, HERE), vec!["codex"]);
    }

    #[test]
    fn notify_only_targets_do_not_consume_the_agent_handoff() {
        let m = msg(
            &["suzy/jessair", "codex"],
            Some(extend(&mint("root", "p1"), "claude")),
        );
        assert_eq!(triggerable_mentions(&m, 20, HERE), vec!["codex"]);
    }

    #[test]
    fn agent_never_triggers_itself() {
        let r = extend(&mint("m0", "p1"), "claude");
        let m = msg(&["claude"], Some(r));
        assert!(triggerable_mentions(&m, 20, HERE).is_empty());
    }

    #[test]
    fn self_mention_is_skipped_not_fatal() {
        // Naming itself first must not consume the one honoured slot.
        let r = extend(&mint("m0", "p1"), "claude");
        let m = msg(&["claude", "codex"], Some(r));
        assert_eq!(triggerable_mentions(&m, 20, HERE), vec!["codex"]);
    }

    #[test]
    fn a_mention_scoped_to_this_device_is_a_self_mention() {
        let r = extend(&mint("m0", "p1"), "claude");
        let m = msg(&["suzy/macbook-pro/claude"], Some(r));
        assert!(triggerable_mentions(&m, 20, HERE).is_empty());
    }

    #[test]
    fn another_devices_agent_of_the_same_name_is_not_itself() {
        // The bug: claude on macbook-pro handing off to claude on jessair was
        // read as a self-mention, so the hand-off silently never fired.
        let r = extend(&mint("m0", "p1"), "claude");
        let m = msg(&["suzy/jessair/claude"], Some(r));
        assert_eq!(
            triggerable_mentions(&m, 20, HERE),
            vec!["suzy/jessair/claude"],
            "a different machine's agent is a different agent"
        );
    }

    #[test]
    fn a_bare_self_name_is_still_skipped() {
        // Unscoped, it resolves to this very agent here — and skipping it frees
        // the one honoured hand-off for the next mention.
        let r = extend(&mint("m0", "p1"), "claude");
        let m = msg(&["claude"], Some(r));
        assert!(triggerable_mentions(&m, 20, HERE).is_empty());
    }

    #[test]
    fn without_knowing_this_device_a_scoped_mention_still_passes() {
        // Roster not synced yet. Refusing would fail closed and break
        // delegation at random; the receiver still catches a true self-wake.
        let r = extend(&mint("m0", "p1"), "claude");
        let m = msg(&["suzy/macbook-pro/claude"], Some(r));
        assert_eq!(
            triggerable_mentions(&m, 20, None),
            vec!["suzy/macbook-pro/claude"]
        );
    }

    #[test]
    fn budget_stops_the_cascade() {
        let mut r = mint("m0", "p1");
        for _ in 0..3 {
            r = extend(&r, "claude");
        }
        let m = msg(&["codex"], Some(r.clone()));
        assert_eq!(r.spent, 3);
        assert!(triggerable_mentions(&m, 3, HERE).is_empty());
        assert_eq!(triggerable_mentions(&m, 4, HERE), vec!["codex"]);
    }

    #[test]
    fn ping_pong_is_allowed_but_bounded() {
        // A -> B -> A is the point of the feature; it terminates on budget.
        let mut r = mint("m0", "p1");
        let mut turns = 0;
        loop {
            let next = if turns % 2 == 0 { "codex" } else { "claude" };
            let posting = if turns % 2 == 0 { "claude" } else { "codex" };
            r = extend(&r, posting);
            let m = msg(&[next], Some(r.clone()));
            if triggerable_mentions(&m, 6, HERE).is_empty() {
                break;
            }
            turns += 1;
            assert!(turns < 100, "cascade did not terminate");
        }
        assert_eq!(turns, 5, "budget of 6 should allow 5 hand-offs");
    }

    #[test]
    fn ceiling_overrides_a_silly_config() {
        let mut r = mint("m0", "p1");
        for _ in 0..RELAY_TURNS_CEILING {
            r = extend(&r, "claude");
        }
        let m = msg(&["codex"], Some(r));
        assert!(triggerable_mentions(&m, 250, HERE).is_empty());
    }

    #[test]
    fn path_records_the_branch() {
        let r = extend(&extend(&mint("m0", "p1"), "claude"), "codex");
        assert_eq!(r.path, vec!["claude", "codex"]);
        assert_eq!(poster(&r), Some("codex"));
        assert_eq!(r.root, "m0");
        assert_eq!(r.root_peer, "p1");
    }
}
