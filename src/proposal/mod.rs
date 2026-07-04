//! Local workspace proposal layer (M14).
//!
//! Agents, editors, and scripts mutate the normal workspace; enoxian captures
//! before/after state as snapshots and turns the difference into reviewable
//! proposals. See `docs/plan/agent-workspaces.md`.
//!
//! This layer sits alongside the CRDT sync watcher (`crate::sync_yjs::watcher`),
//! not in place of it: the CRDT watcher serves interactive editing, while this
//! layer treats the same file events as proposal evidence.

pub mod adapters;
pub mod blob;
pub mod diff;
pub mod engine;
pub mod journal;
pub mod merge;
pub mod model;
pub mod policy;
pub mod session;
pub mod snapshot;
pub mod store;
pub mod sync;
