//! Proposal acceptance policy.
//!
//! Local triggers default to auto-accept with full history and revert
//! (git-like: commits land, the log is always there, revert is always
//! available). Remote-member triggers default to pending review.
//!
//! Auto-accept is only safe once the undo path is solid: the blob store,
//! snapshot diff, and revert command must exist before auto-accept is
//! enabled by default.

use serde::{Deserialize, Serialize};

/// Who caused the change session behind a proposal, as far as the local
/// daemon can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerOrigin {
    /// The local user triggered a local agent (or edited directly).
    LocalUser,
    /// Another circle member's mention triggered a local agent.
    RemoteMember,
    /// Ambient change with no attribution.
    Unattributed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptAction {
    /// Apply to the canonical state immediately; keep the history entry so
    /// the change can be viewed and reverted at any time.
    AutoAccept,
    /// Hold the proposal for explicit review.
    PendingReview,
}

/// Daemon-local configuration. Like the agent registry, this is never
/// synced: a remote peer cannot loosen another device's acceptance policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptancePolicy {
    /// Auto-accept proposals from sessions the local user started.
    #[serde(default = "default_true")]
    pub auto_accept_local: bool,
    /// Auto-accept proposals triggered by remote circle members.
    #[serde(default)]
    pub auto_accept_remote: bool,
}

fn default_true() -> bool {
    true
}

impl Default for AcceptancePolicy {
    fn default() -> Self {
        Self {
            auto_accept_local: true,
            auto_accept_remote: false,
        }
    }
}

impl AcceptancePolicy {
    pub fn decide(&self, origin: TriggerOrigin) -> AcceptAction {
        match origin {
            TriggerOrigin::LocalUser if self.auto_accept_local => AcceptAction::AutoAccept,
            TriggerOrigin::RemoteMember if self.auto_accept_remote => AcceptAction::AutoAccept,
            // Unattributed ambient work is kept, never auto-merged.
            _ => AcceptAction::PendingReview,
        }
    }

    // TODO(M14): managed process mode (`enox agent run` with optional
    // sandbox), proposal history/revert frontend view, and per-agent or
    // per-member overrides (open question in the design doc).
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_design_doc() {
        let policy = AcceptancePolicy::default();
        assert_eq!(
            policy.decide(TriggerOrigin::LocalUser),
            AcceptAction::AutoAccept
        );
        assert_eq!(
            policy.decide(TriggerOrigin::RemoteMember),
            AcceptAction::PendingReview
        );
        assert_eq!(
            policy.decide(TriggerOrigin::Unattributed),
            AcceptAction::PendingReview
        );
    }

    #[test]
    fn remote_auto_accept_is_opt_in() {
        let policy = AcceptancePolicy {
            auto_accept_local: true,
            auto_accept_remote: true,
        };
        assert_eq!(
            policy.decide(TriggerOrigin::RemoteMember),
            AcceptAction::AutoAccept
        );
        // Unattributed stays pending even with permissive settings.
        assert_eq!(
            policy.decide(TriggerOrigin::Unattributed),
            AcceptAction::PendingReview
        );
    }

    #[test]
    fn parses_from_toml_with_defaults() {
        let policy: AcceptancePolicy = toml::from_str("").unwrap();
        assert!(policy.auto_accept_local);
        assert!(!policy.auto_accept_remote);
    }
}
