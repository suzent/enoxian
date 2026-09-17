//! Shared daemon state — holds one AppState per active circle.

use crate::state::AppState;
use dashmap::DashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Top-level state threaded through all axum handlers.
/// Clone is cheap — all fields are Arc.
#[derive(Clone)]
pub struct DaemonState {
    /// circle_id → per-circle runtime state (active circles only)
    pub circles: Arc<DashMap<String, AppState>>,
    /// circle_id → cancellation token for all tasks belonging to that circle
    pub tokens: Arc<DashMap<String, CancellationToken>>,
    /// circle_ids whose startup is in flight. A circle is not yet in `circles`
    /// while it loads, so without this every caller that checks `is_active`
    /// would start a second copy of one that is merely slow.
    starting: Arc<DashMap<String, ()>>,
    /// workspace key → the circle_id that owns that directory. Claimed before a
    /// circle loads rather than checked against `circles`, which is only
    /// populated once it has finished.
    workspaces: Arc<DashMap<String, String>>,
    /// Cancelled when `POST /shutdown` is called — triggers graceful server exit.
    pub shutdown_token: CancellationToken,
}

/// Holds a workspace for one circle. Released when startup fails or the guard
/// is dropped, unless [`WorkspaceClaim::retain`] hands it to the running circle.
pub struct WorkspaceClaim {
    workspaces: Arc<DashMap<String, String>>,
    key: String,
    retain: bool,
}

impl WorkspaceClaim {
    /// Keep the claim for as long as the circle runs; `stop_circle` releases it.
    pub fn retain(mut self) {
        self.retain = true;
    }
}

impl Drop for WorkspaceClaim {
    fn drop(&mut self) {
        if !self.retain {
            self.workspaces.remove(&self.key);
        }
    }
}

/// Releases a circle's start reservation when startup finishes or panics.
pub struct StartGuard {
    starting: Arc<DashMap<String, ()>>,
    circle_id: String,
}

impl Drop for StartGuard {
    fn drop(&mut self) {
        self.starting.remove(&self.circle_id);
    }
}

impl Default for DaemonState {
    fn default() -> Self {
        Self::new()
    }
}

impl DaemonState {
    pub fn new() -> Self {
        Self {
            circles: Arc::new(DashMap::new()),
            tokens: Arc::new(DashMap::new()),
            starting: Arc::new(DashMap::new()),
            workspaces: Arc::new(DashMap::new()),
            shutdown_token: CancellationToken::new(),
        }
    }

    /// Reserve a circle for startup, or return `None` if it is already active
    /// or already starting. Hold the guard for the duration of the start.
    pub fn begin_start(&self, circle_id: &str) -> Option<StartGuard> {
        if self.is_active(circle_id) {
            return None;
        }
        // DashMap::insert returns the previous value, so a non-empty return
        // means another caller reserved this circle first.
        if self.starting.insert(circle_id.to_string(), ()).is_some() {
            return None;
        }
        Some(StartGuard {
            starting: self.starting.clone(),
            circle_id: circle_id.to_string(),
        })
    }

    pub fn insert(&self, circle_id: String, state: AppState) {
        self.circles.insert(circle_id, state);
    }

    pub fn insert_circle(&self, circle_id: String, state: AppState, token: CancellationToken) {
        self.circles.insert(circle_id.clone(), state);
        self.tokens.insert(circle_id, token);
    }

    /// Claim a workspace directory for a circle, or return the circle_id that
    /// already owns it.
    ///
    /// Checking the active set cannot do this job: circles register only once
    /// they have finished loading, so two circles configured against the same
    /// directory would both look unopposed and both start watching it.
    pub fn claim_workspace(&self, key: String, circle_id: &str) -> Result<WorkspaceClaim, String> {
        use dashmap::mapref::entry::Entry;
        match self.workspaces.entry(key.clone()) {
            Entry::Occupied(held) if held.get() != circle_id => Err(held.get().clone()),
            Entry::Occupied(_) => Ok(WorkspaceClaim {
                workspaces: self.workspaces.clone(),
                key,
                retain: false,
            }),
            Entry::Vacant(slot) => {
                slot.insert(circle_id.to_string());
                Ok(WorkspaceClaim {
                    workspaces: self.workspaces.clone(),
                    key,
                    retain: false,
                })
            }
        }
    }

    /// Cancel all tasks for a circle and remove it from the active set.
    /// Returns true if the circle was active.
    pub fn stop_circle(&self, circle_id: &str) -> bool {
        if let Some((_, token)) = self.tokens.remove(circle_id) {
            token.cancel();
        }
        self.workspaces.retain(|_, owner| owner != circle_id);
        self.circles.remove(circle_id).is_some()
    }

    /// Signal every daemon-owned task and connection to shut down.
    pub fn shutdown(&self) {
        self.shutdown_token.cancel();
        for token in self.tokens.iter() {
            token.value().cancel();
        }
    }

    pub fn get(&self, circle_id: &str) -> Option<AppState> {
        self.circles.get(circle_id).map(|r| r.clone())
    }

    pub fn list(&self) -> Vec<AppState> {
        self.circles.iter().map(|r| r.value().clone()).collect()
    }

    pub fn is_active(&self, circle_id: &str) -> bool {
        self.circles.contains_key(circle_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_circles_cannot_start_against_one_workspace() {
        // The case serial startup used to cover: neither circle is active yet,
        // so an "is anyone else using this?" scan sees nothing either way.
        let daemon = DaemonState::new();
        let claim = daemon
            .claim_workspace("/w".to_string(), "circle-a")
            .expect("first claim");
        assert_eq!(
            daemon
                .claim_workspace("/w".to_string(), "circle-b")
                .err()
                .as_deref(),
            Some("circle-a")
        );
        // A failed start releases, so the directory is not stranded.
        drop(claim);
        assert!(daemon.claim_workspace("/w".to_string(), "circle-b").is_ok());
    }

    #[test]
    fn stopping_a_circle_frees_its_workspace_for_another() {
        let daemon = DaemonState::new();
        daemon
            .claim_workspace("/w".to_string(), "circle-a")
            .expect("claim")
            .retain();
        assert!(daemon
            .claim_workspace("/w".to_string(), "circle-b")
            .is_err());
        daemon.stop_circle("circle-a");
        assert!(daemon.claim_workspace("/w".to_string(), "circle-b").is_ok());
    }

    #[test]
    fn a_circle_is_only_reserved_for_one_start_at_a_time() {
        let daemon = DaemonState::new();
        let guard = daemon.begin_start("circle").expect("first reservation");
        assert!(
            daemon.begin_start("circle").is_none(),
            "a slow start must not be started again"
        );
        drop(guard);
        assert!(daemon.begin_start("circle").is_some(), "released on drop");
    }

    #[test]
    fn shutdown_cancels_daemon_and_circle_tokens() {
        let daemon = DaemonState::new();
        let circle_token = CancellationToken::new();
        daemon
            .tokens
            .insert("circle".to_string(), circle_token.clone());

        daemon.shutdown();

        assert!(daemon.shutdown_token.is_cancelled());
        assert!(circle_token.is_cancelled());
    }
}
