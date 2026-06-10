//! Daemon-side trigger handling: the execution boundary.
//!
//! The daemon on the target device decides whether to honor a replicated
//! trigger, how to launch the agent, and what session to open. Remote users
//! can request work; only the local daemon runs local agents.

use super::registry::AgentRegistry;
use super::{AgentTriggered, TriggerStatus};

/// Decides how to respond to a replicated `agent_triggered` event.
///
/// `requested_agent` is a routing hint; the registry (allowlist) is the
/// security gate.
pub fn handle_trigger(registry: &AgentRegistry, event: &AgentTriggered) -> TriggerStatus {
    let Some(_cmd) = registry.resolve(&event.requested_agent) else {
        return TriggerStatus::Ignored;
    };

    // TODO(M14): verify event.requested_by against the current signed member
    //            list before acting.
    // TODO(M14): open a LocalChangeSession (mode: ambient_triggered) at the
    //            current base snapshot, then spawn cmd.render(&event.task_text)
    //            in the workspace.
    // TODO(M14): emit TriggerStatusReply (started/failed) back to the circle,
    //            and completed/expired when the session closes.
    TriggerStatus::Delivered
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(agent: &str) -> AgentTriggered {
        AgentTriggered::new(
            "circle-1".into(),
            agent.into(),
            "fix the docs".into(),
            "peer-abc".into(),
            "msg-1".into(),
        )
    }

    #[test]
    fn unregistered_agent_is_ignored() {
        let registry = AgentRegistry::default();
        assert_eq!(handle_trigger(&registry, &event("codex")), TriggerStatus::Ignored);
    }

    #[test]
    fn registered_agent_is_delivered() {
        let registry = AgentRegistry::from_toml(
            "[agents.codex]\ncommand = [\"codex\", \"{{task}}\"]",
        )
        .unwrap();
        assert_eq!(handle_trigger(&registry, &event("codex")), TriggerStatus::Delivered);
    }
}
