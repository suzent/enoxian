//! Shared daemon state — holds one AppState per active circle.

use std::sync::Arc;
use dashmap::DashMap;
use crate::state::AppState;

/// Top-level state threaded through all axum handlers.
/// Clone is cheap — all fields are Arc.
#[derive(Clone)]
pub struct DaemonState {
    /// circle_id → per-circle runtime state
    pub circles: Arc<DashMap<String, AppState>>,
}

impl DaemonState {
    pub fn new() -> Self {
        Self { circles: Arc::new(DashMap::new()) }
    }

    pub fn insert(&self, circle_id: String, state: AppState) {
        self.circles.insert(circle_id, state);
    }

    pub fn get(&self, circle_id: &str) -> Option<AppState> {
        self.circles.get(circle_id).map(|r| r.clone())
    }

    pub fn list(&self) -> Vec<AppState> {
        self.circles.iter().map(|r| r.value().clone()).collect()
    }
}
