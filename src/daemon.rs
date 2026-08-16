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
    /// Cancelled when `POST /shutdown` is called — triggers graceful server exit.
    pub shutdown_token: CancellationToken,
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
            shutdown_token: CancellationToken::new(),
        }
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
