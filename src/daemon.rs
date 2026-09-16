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
    /// Cancelled when `POST /shutdown` is called — triggers graceful server exit.
    pub shutdown_token: CancellationToken,
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

    /// Cancel all tasks for a circle and remove it from the active set.
    /// Returns true if the circle was active.
    pub fn stop_circle(&self, circle_id: &str) -> bool {
        if let Some((_, token)) = self.tokens.remove(circle_id) {
            token.cancel();
        }
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
