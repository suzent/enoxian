# Workspace Event Log (M15)

**Status:** Implemented.

The workspace event log is the durable coordination record above immutable
snapshot manifests and content-addressed blobs. It does not proxy file I/O:
agents and editors still write the native workspace, and the proposal engine
captures their effects.

## Storage and schema

Events are stored individually under `.enox_events/events/<uuid>.json`. Every
event carries:

- schema version, UUID, circle id, origin peer/device, and timestamp;
- direct causal parent event ids;
- a Lamport time derived from its parents;
- one typed payload: workspace fork, snapshot, proposal creation, proposal
  status change, rejection, completed merge, or detected conflict.

Events are immutable. Replaying an identical id is a no-op; reusing an id with
different content is rejected. Paths and all proposal/snapshot/event ids are
validated before persistence.

## Materialization

`EventStore::materialize` deterministically sorts the event set by Lamport time
and stable tie-breakers, then derives:

- the current snapshot manifest id;
- proposal base/result snapshots and winning status;
- workspace forks;
- completed merges;
- per-proposal conflict paths;
- the causal frontier for the next local event.

Concurrent proposal decisions use the same explicit status precedence as the
proposal model (`reverted > rejected > accepted/synced > conflicted > pending`),
then causal/stable ordering. `materialized_snapshot` resolves the selected id to
the real immutable manifest in the proposal store.

On upgrade, proposal records created before M15 are backfilled into event
history before the proposal engine starts. Backfill is idempotent locally and
safe when multiple peers independently backfill the same proposal: event sets
merge by union and status precedence still converges.

## Peer protocol

`/enoxian/events/1.0.0` performs an initial id-based anti-entropy exchange and
then stays open. New events are forwarded immediately, so status decisions and
conflict metadata do not wait for a reconnect.

Proposal-related events carry the existing `ProposalBundle`; snapshot manifests
and embedded blobs therefore arrive with live event history. Initial histories
are sent as individually bounded frames followed by `EventsDone`, avoiding an
unbounded all-events frame. Each phase rechecks the membership tombstone gate,
and a live stream terminates when either endpoint is removed.

The older proposal pull protocol remains as compatibility and missing-large-
blob reconciliation. After either protocol applies proposal metadata, mutable
legacy proposal records are reconciled from the authoritative event-log
materialization so existing API and frontend readers continue to work.

## Runtime producers

- The proposal engine records snapshot and proposal-created events.
- Accept records a proposal-status event.
- Reject records the exact post-reverse-apply snapshot and a rejection event.
- Revert records the exact snapshot and status event.
- Reject/revert record completed merge events.
- A three-way reverse-apply conflict records the conflicting paths before the
  API returns `409 Conflict`.

The schema also includes workspace-fork events for the optional fork/sandbox
mode deferred from M14.
