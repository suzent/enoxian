//! Durable, append-only workspace event log (M15).
//!
//! The proposal store owns content and snapshot manifests. This log records the
//! causal decisions that move workspace state between those immutable
//! snapshots. Events are immutable and synchronize by id, so peers converge by
//! set union instead of overwriting mutable proposal records.

use crate::proposal::model::ProposalStatus;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const STORE_DIR: &str = ".enox_events";
pub const SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceEvent {
    pub schema_version: u16,
    pub id: String,
    pub circle_id: String,
    /// Direct causal predecessors. Concurrent events have disjoint parents.
    pub parents: Vec<String>,
    /// Lamport time: one greater than the greatest parent time.
    pub lamport: u64,
    pub origin_peer_id: String,
    pub origin_device: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub kind: WorkspaceEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkspaceEventKind {
    WorkspaceForked {
        fork_id: String,
        base_snapshot: String,
    },
    SnapshotRecorded {
        snapshot_id: String,
        parent_snapshot: Option<String>,
    },
    ProposalCreated {
        proposal_id: String,
        base_snapshot: String,
        result_snapshot: String,
        changed_paths: Vec<String>,
        initial_status: ProposalStatus,
    },
    ProposalStatusChanged {
        proposal_id: String,
        status: ProposalStatus,
        /// Snapshot that was actually present after the decision. This matters
        /// for reverse-applied rejects/reverts that preserve later edits.
        materialized_snapshot: String,
    },
    ProposalRejected {
        proposal_id: String,
        materialized_snapshot: String,
    },
    MergeCompleted {
        proposal_ids: Vec<String>,
        result_snapshot: String,
    },
    ConflictDetected {
        proposal_id: String,
        paths: Vec<String>,
    },
}

impl WorkspaceEventKind {
    pub fn proposal_id(&self) -> Option<&str> {
        match self {
            Self::ProposalCreated { proposal_id, .. }
            | Self::ProposalStatusChanged { proposal_id, .. }
            | Self::ProposalRejected { proposal_id, .. }
            | Self::ConflictDetected { proposal_id, .. } => Some(proposal_id),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterializedProposal {
    pub base_snapshot: String,
    pub result_snapshot: String,
    pub status: ProposalStatus,
    pub conflicts: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterializedWorkspace {
    pub current_snapshot: Option<String>,
    pub proposals: BTreeMap<String, MaterializedProposal>,
    pub forks: BTreeMap<String, String>,
    pub completed_merges: Vec<(Vec<String>, String)>,
    pub frontier: Vec<String>,
}

pub struct EventStore {
    root: PathBuf,
    circle_id: String,
}

impl EventStore {
    pub fn open(workspace: &Path, circle_id: impl Into<String>) -> Result<Self> {
        let root = workspace.join(STORE_DIR);
        std::fs::create_dir_all(root.join("events"))?;
        Ok(Self {
            root,
            circle_id: circle_id.into(),
        })
    }

    fn events_dir(&self) -> PathBuf {
        self.root.join("events")
    }

    pub fn contains(&self, id: &str) -> bool {
        self.events_dir().join(format!("{id}.json")).is_file()
    }

    pub fn load(&self, id: &str) -> Result<WorkspaceEvent> {
        validate_id("event", id)?;
        let path = self.events_dir().join(format!("{id}.json"));
        let bytes = std::fs::read(&path)
            .with_context(|| format!("reading workspace event {}", path.display()))?;
        let event: WorkspaceEvent = serde_json::from_slice(&bytes)?;
        self.validate(&event)?;
        Ok(event)
    }

    pub fn list(&self) -> Vec<WorkspaceEvent> {
        let mut events: Vec<_> = std::fs::read_dir(self.events_dir())
            .map(|entries| {
                entries
                    .filter_map(|entry| {
                        let bytes = std::fs::read(entry.ok()?.path()).ok()?;
                        let event: WorkspaceEvent = serde_json::from_slice(&bytes).ok()?;
                        self.validate(&event).ok()?;
                        Some(event)
                    })
                    .collect()
            })
            .unwrap_or_default();
        events.sort_by_key(order_key);
        events
    }

    pub fn ids(&self) -> Vec<String> {
        self.list().into_iter().map(|event| event.id).collect()
    }

    pub fn frontier(&self) -> Vec<String> {
        frontier(&self.list())
    }

    pub fn append_local(
        &self,
        origin_peer_id: impl Into<String>,
        origin_device: impl Into<String>,
        kind: WorkspaceEventKind,
    ) -> Result<WorkspaceEvent> {
        let existing = self.list();
        let parents = frontier(&existing);
        let lamport = existing
            .iter()
            .map(|event| event.lamport)
            .max()
            .unwrap_or(0)
            + 1;
        let event = WorkspaceEvent {
            schema_version: SCHEMA_VERSION,
            id: uuid::Uuid::new_v4().to_string(),
            circle_id: self.circle_id.clone(),
            parents,
            lamport,
            origin_peer_id: origin_peer_id.into(),
            origin_device: origin_device.into(),
            created_at: chrono::Utc::now(),
            kind,
        };
        self.append(&event)?;
        Ok(event)
    }

    /// Append a local or remote event. Returns false for an identical replay.
    pub fn append(&self, event: &WorkspaceEvent) -> Result<bool> {
        self.validate(event)?;
        let path = self.events_dir().join(format!("{}.json", event.id));
        if path.exists() {
            let existing: WorkspaceEvent = serde_json::from_slice(&std::fs::read(&path)?)?;
            if existing == *event {
                return Ok(false);
            }
            bail!("workspace event id collision: {}", event.id);
        }
        let temp = self.events_dir().join(format!(".{}.tmp", event.id));
        std::fs::write(&temp, serde_json::to_vec_pretty(event)?)?;
        std::fs::rename(&temp, &path)
            .with_context(|| format!("committing workspace event {}", event.id))?;
        Ok(true)
    }

    pub fn materialize(&self) -> MaterializedWorkspace {
        materialize(&self.list())
    }

    /// Load the immutable manifest selected by event-log materialization.
    pub fn materialized_snapshot(
        &self,
        proposals: &crate::proposal::store::ProposalStore,
    ) -> Result<Option<crate::proposal::snapshot::Snapshot>> {
        self.materialize()
            .current_snapshot
            .map(|id| proposals.load_snapshot(&id))
            .transpose()
    }

    /// One-time/upgrade migration for proposal records created before M15.
    /// Duplicate backfills from different peers are harmless: events converge
    /// by union and proposal status precedence is deterministic.
    pub fn backfill_proposals(
        &self,
        proposals: &crate::proposal::store::ProposalStore,
        origin_peer_id: &str,
        origin_device: &str,
    ) -> Result<Vec<WorkspaceEvent>> {
        let represented: BTreeSet<String> = self
            .list()
            .into_iter()
            .filter_map(|event| match event.kind {
                WorkspaceEventKind::ProposalCreated { proposal_id, .. } => Some(proposal_id),
                _ => None,
            })
            .collect();
        let mut appended = Vec::new();
        for proposal in proposals.list_proposals().into_iter().rev() {
            if represented.contains(&proposal.id) {
                continue;
            }
            appended.push(self.append_local(
                origin_peer_id,
                origin_device,
                WorkspaceEventKind::SnapshotRecorded {
                    snapshot_id: proposal.result_snapshot.clone(),
                    parent_snapshot: Some(proposal.base_snapshot.clone()),
                },
            )?);
            appended.push(self.append_local(
                origin_peer_id,
                origin_device,
                WorkspaceEventKind::ProposalCreated {
                    proposal_id: proposal.id,
                    base_snapshot: proposal.base_snapshot,
                    result_snapshot: proposal.result_snapshot,
                    changed_paths: proposal.changed_paths,
                    initial_status: proposal.status,
                },
            )?);
        }
        Ok(appended)
    }

    fn validate(&self, event: &WorkspaceEvent) -> Result<()> {
        if event.schema_version != SCHEMA_VERSION {
            bail!(
                "unsupported workspace event schema {}",
                event.schema_version
            );
        }
        validate_id("event", &event.id)?;
        if event.circle_id != self.circle_id {
            bail!(
                "workspace event {} belongs to circle {}, not {}",
                event.id,
                event.circle_id,
                self.circle_id
            );
        }
        for parent in &event.parents {
            validate_id("event parent", parent)?;
            if parent == &event.id {
                bail!("workspace event cannot parent itself");
            }
        }
        validate_kind(&event.kind)
    }
}

/// Append an event for the local daemon and wake live event-sync streams.
pub fn append_local_event(
    state: &crate::state::AppState,
    origin_device: impl Into<String>,
    kind: WorkspaceEventKind,
) -> Result<WorkspaceEvent> {
    let store = EventStore::open(&state.workspace, state.circle_id.clone())?;
    let event = store.append_local(state.peer_id.clone(), origin_device, kind)?;
    let _ = state
        .events
        .send(crate::control::CircleEvent::WorkspaceEventAppended {
            event_id: event.id.clone(),
        });
    Ok(event)
}

fn validate_id(kind: &str, id: &str) -> Result<()> {
    uuid::Uuid::parse_str(id).map_err(|_| anyhow::anyhow!("invalid {kind} id: {id}"))?;
    Ok(())
}

fn validate_snapshot(id: &str) -> Result<()> {
    crate::proposal::validate_storage_id("snapshot", id)
}

fn validate_kind(kind: &WorkspaceEventKind) -> Result<()> {
    match kind {
        WorkspaceEventKind::WorkspaceForked {
            fork_id,
            base_snapshot,
        } => {
            validate_id("fork", fork_id)?;
            validate_snapshot(base_snapshot)?;
        }
        WorkspaceEventKind::SnapshotRecorded {
            snapshot_id,
            parent_snapshot,
        } => {
            validate_snapshot(snapshot_id)?;
            if let Some(parent) = parent_snapshot {
                validate_snapshot(parent)?;
            }
        }
        WorkspaceEventKind::ProposalCreated {
            proposal_id,
            base_snapshot,
            result_snapshot,
            changed_paths,
            ..
        } => {
            validate_id("proposal", proposal_id)?;
            validate_snapshot(base_snapshot)?;
            validate_snapshot(result_snapshot)?;
            for path in changed_paths {
                crate::proposal::validate_workspace_path(path)?;
            }
        }
        WorkspaceEventKind::ProposalStatusChanged {
            proposal_id,
            materialized_snapshot,
            ..
        }
        | WorkspaceEventKind::ProposalRejected {
            proposal_id,
            materialized_snapshot,
        } => {
            validate_id("proposal", proposal_id)?;
            validate_snapshot(materialized_snapshot)?;
        }
        WorkspaceEventKind::MergeCompleted {
            proposal_ids,
            result_snapshot,
        } => {
            for id in proposal_ids {
                validate_id("proposal", id)?;
            }
            validate_snapshot(result_snapshot)?;
        }
        WorkspaceEventKind::ConflictDetected { proposal_id, paths } => {
            validate_id("proposal", proposal_id)?;
            for path in paths {
                crate::proposal::validate_workspace_path(path)?;
            }
        }
    }
    Ok(())
}

fn order_key(event: &WorkspaceEvent) -> (u64, chrono::DateTime<chrono::Utc>, String, String) {
    (
        event.lamport,
        event.created_at,
        event.origin_peer_id.clone(),
        event.id.clone(),
    )
}

fn frontier(events: &[WorkspaceEvent]) -> Vec<String> {
    let mut ids: BTreeSet<_> = events.iter().map(|event| event.id.clone()).collect();
    for parent in events.iter().flat_map(|event| event.parents.iter()) {
        ids.remove(parent);
    }
    ids.into_iter().collect()
}

pub fn materialize(events: &[WorkspaceEvent]) -> MaterializedWorkspace {
    let mut sorted = events.to_vec();
    sorted.sort_by_key(order_key);
    let mut state = MaterializedWorkspace::default();
    let mut status_keys: BTreeMap<String, (u8, u64, String, String)> = BTreeMap::new();

    for event in &sorted {
        let event_key = |status: ProposalStatus| {
            (
                status.rank(),
                event.lamport,
                event.origin_peer_id.clone(),
                event.id.clone(),
            )
        };
        match &event.kind {
            WorkspaceEventKind::WorkspaceForked {
                fork_id,
                base_snapshot,
            } => {
                state.forks.insert(fork_id.clone(), base_snapshot.clone());
            }
            WorkspaceEventKind::SnapshotRecorded { snapshot_id, .. } => {
                state.current_snapshot = Some(snapshot_id.clone());
            }
            WorkspaceEventKind::ProposalCreated {
                proposal_id,
                base_snapshot,
                result_snapshot,
                initial_status,
                ..
            } => {
                let key = event_key(*initial_status);
                let proposal = state
                    .proposals
                    .entry(proposal_id.clone())
                    .or_insert_with(|| MaterializedProposal {
                        base_snapshot: base_snapshot.clone(),
                        result_snapshot: result_snapshot.clone(),
                        status: *initial_status,
                        conflicts: Vec::new(),
                    });
                if status_keys
                    .get(proposal_id)
                    .is_none_or(|current| key > *current)
                {
                    status_keys.insert(proposal_id.clone(), key);
                    proposal.status = *initial_status;
                }
                if proposal.status == *initial_status
                    && matches!(
                        initial_status,
                        ProposalStatus::Accepted | ProposalStatus::Synced
                    )
                {
                    state.current_snapshot = Some(result_snapshot.clone());
                }
            }
            WorkspaceEventKind::ProposalStatusChanged {
                proposal_id,
                status,
                materialized_snapshot,
            } => {
                let key = event_key(*status);
                if status_keys
                    .get(proposal_id)
                    .is_none_or(|current| key > *current)
                {
                    status_keys.insert(proposal_id.clone(), key);
                    if let Some(proposal) = state.proposals.get_mut(proposal_id) {
                        proposal.status = *status;
                    }
                    state.current_snapshot = Some(materialized_snapshot.clone());
                }
            }
            WorkspaceEventKind::ProposalRejected {
                proposal_id,
                materialized_snapshot,
            } => {
                let status = ProposalStatus::Rejected;
                let key = event_key(status);
                if status_keys
                    .get(proposal_id)
                    .is_none_or(|current| key > *current)
                {
                    status_keys.insert(proposal_id.clone(), key);
                    if let Some(proposal) = state.proposals.get_mut(proposal_id) {
                        proposal.status = status;
                    }
                    state.current_snapshot = Some(materialized_snapshot.clone());
                }
            }
            WorkspaceEventKind::MergeCompleted {
                proposal_ids,
                result_snapshot,
            } => {
                state
                    .completed_merges
                    .push((proposal_ids.clone(), result_snapshot.clone()));
                state.current_snapshot = Some(result_snapshot.clone());
            }
            WorkspaceEventKind::ConflictDetected { proposal_id, paths } => {
                if let Some(proposal) = state.proposals.get_mut(proposal_id) {
                    proposal.conflicts.extend(paths.iter().cloned());
                    proposal.conflicts.sort();
                    proposal.conflicts.dedup();
                }
            }
        }
    }
    state.frontier = frontier(&sorted);
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proposal::model::Proposal;
    use crate::proposal::snapshot::Snapshot;
    use crate::proposal::store::ProposalStore;

    fn snapshot_id() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    #[test]
    fn append_is_idempotent_and_materializes_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let store = EventStore::open(dir.path(), "circle").unwrap();
        let snapshot = snapshot_id();
        let event = store
            .append_local(
                "peer-a",
                "laptop",
                WorkspaceEventKind::SnapshotRecorded {
                    snapshot_id: snapshot.clone(),
                    parent_snapshot: None,
                },
            )
            .unwrap();
        assert!(!store.append(&event).unwrap());
        assert_eq!(store.materialize().current_snapshot, Some(snapshot));
        assert_eq!(store.frontier(), vec![event.id]);
    }

    #[test]
    fn status_conflicts_use_proposal_precedence_not_arrival_order() {
        let base = snapshot_id();
        let result = snapshot_id();
        let proposal_id = uuid::Uuid::new_v4().to_string();
        let dir = tempfile::tempdir().unwrap();
        let store = EventStore::open(dir.path(), "circle").unwrap();
        store
            .append_local(
                "peer-a",
                "a",
                WorkspaceEventKind::ProposalCreated {
                    proposal_id: proposal_id.clone(),
                    base_snapshot: base,
                    result_snapshot: result.clone(),
                    changed_paths: vec!["src/main.rs".into()],
                    initial_status: ProposalStatus::Pending,
                },
            )
            .unwrap();
        let rejected_snapshot = snapshot_id();
        store
            .append_local(
                "peer-a",
                "a",
                WorkspaceEventKind::ProposalRejected {
                    proposal_id: proposal_id.clone(),
                    materialized_snapshot: rejected_snapshot.clone(),
                },
            )
            .unwrap();
        store
            .append_local(
                "peer-b",
                "b",
                WorkspaceEventKind::ProposalStatusChanged {
                    proposal_id: proposal_id.clone(),
                    status: ProposalStatus::Accepted,
                    materialized_snapshot: result,
                },
            )
            .unwrap();
        let materialized = store.materialize();
        assert_eq!(
            materialized.proposals[&proposal_id].status,
            ProposalStatus::Rejected
        );
        assert_eq!(materialized.current_snapshot, Some(rejected_snapshot));
    }

    #[test]
    fn union_converges_independent_of_arrival_order_and_syncs_conflicts() {
        let base = snapshot_id();
        let result = snapshot_id();
        let proposal_id = uuid::Uuid::new_v4().to_string();
        let mk = |id: String, lamport, kind| WorkspaceEvent {
            schema_version: SCHEMA_VERSION,
            id,
            circle_id: "circle".into(),
            parents: Vec::new(),
            lamport,
            origin_peer_id: "peer".into(),
            origin_device: "device".into(),
            created_at: chrono::Utc::now(),
            kind,
        };
        let created = mk(
            uuid::Uuid::new_v4().to_string(),
            1,
            WorkspaceEventKind::ProposalCreated {
                proposal_id: proposal_id.clone(),
                base_snapshot: base,
                result_snapshot: result,
                changed_paths: vec!["a.txt".into()],
                initial_status: ProposalStatus::Accepted,
            },
        );
        let conflict = mk(
            uuid::Uuid::new_v4().to_string(),
            2,
            WorkspaceEventKind::ConflictDetected {
                proposal_id: proposal_id.clone(),
                paths: vec!["a.txt".into()],
            },
        );
        assert_eq!(
            materialize(&[created.clone(), conflict.clone()]),
            materialize(&[conflict, created])
        );
        assert_eq!(
            materialize(&store_events(&proposal_id)).proposals[&proposal_id].conflicts,
            vec!["a.txt"]
        );
    }

    #[test]
    fn backfill_is_idempotent_and_materializes_real_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let proposals = ProposalStore::open(dir.path()).unwrap();
        let base = Snapshot::new(BTreeMap::new());
        let result = Snapshot::new(BTreeMap::new());
        proposals.save_snapshot(&base).unwrap();
        proposals.save_snapshot(&result).unwrap();
        let proposal = Proposal::ambient(
            "circle".into(),
            base.id,
            result.id.clone(),
            vec!["a.txt".into()],
        );
        proposals.save_proposal(&proposal).unwrap();

        let events = EventStore::open(dir.path(), "circle").unwrap();
        assert_eq!(
            events
                .backfill_proposals(&proposals, "peer", "device")
                .unwrap()
                .len(),
            2
        );
        assert!(events
            .backfill_proposals(&proposals, "peer", "device")
            .unwrap()
            .is_empty());
        assert_eq!(
            events
                .materialized_snapshot(&proposals)
                .unwrap()
                .unwrap()
                .id,
            result.id
        );
    }

    fn store_events(proposal_id: &str) -> Vec<WorkspaceEvent> {
        let now = chrono::Utc::now();
        vec![
            WorkspaceEvent {
                schema_version: SCHEMA_VERSION,
                id: uuid::Uuid::new_v4().to_string(),
                circle_id: "circle".into(),
                parents: Vec::new(),
                lamport: 1,
                origin_peer_id: "peer".into(),
                origin_device: "device".into(),
                created_at: now,
                kind: WorkspaceEventKind::ProposalCreated {
                    proposal_id: proposal_id.into(),
                    base_snapshot: snapshot_id(),
                    result_snapshot: snapshot_id(),
                    changed_paths: vec!["a.txt".into()],
                    initial_status: ProposalStatus::Accepted,
                },
            },
            WorkspaceEvent {
                schema_version: SCHEMA_VERSION,
                id: uuid::Uuid::new_v4().to_string(),
                circle_id: "circle".into(),
                parents: Vec::new(),
                lamport: 2,
                origin_peer_id: "peer".into(),
                origin_device: "device".into(),
                created_at: now,
                kind: WorkspaceEventKind::ConflictDetected {
                    proposal_id: proposal_id.into(),
                    paths: vec!["a.txt".into()],
                },
            },
        ]
    }
}
