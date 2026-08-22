# Implementation Internals

This document covers mechanics that are useful when changing the daemon. For the public model, start with [Architecture](architecture.md).

## Workspace Documents

The daemon maintains one Yjs document per synchronized path. Text content is stored in a `Y.Text`; binary content uses the corresponding byte representation. Per-file documents limit conflict and update scope while the circle control document carries non-file collaboration state.

Persisted Yjs updates live under `.enox_crdt/`. The store loads them before live synchronization starts so a restarted daemon can reconstruct its local state without another peer.

## File Watcher Loop

The watcher observes creates, writes, renames, and removals beneath the Circle workspace. Internal enoxian metadata paths are excluded.

For a local change it:

1. normalizes the relative path;
2. reads the resulting file content through native filesystem APIs;
3. updates the matching Yjs document;
4. records accepted proposal history and a causal event;
5. sends the update to connected peers.

Remote synchronization writes the converged result to disk. Expected-write suppression prevents that materialization from being treated as a new local edit and echoed indefinitely.

## Attribution

Managed agent sessions can associate filesystem activity with an agent and task. When no reliable agent context exists, the daemon attributes the change to the local device or an unknown actor rather than inventing ownership.

Attribution describes observed origin; it is not a security boundary. The proposal snapshot and causal event log preserve it for later inspection.

## Proposal Capture

Proposal capture runs alongside the live CRDT update. A captured snapshot points to the preceding accepted version of the path. Ordinary workspace edits are stored with accepted status immediately. Diff and three-way merge use the snapshot ancestry, and revert creates a new accepted version rather than deleting history.

Large contents may be stored as content-addressed blobs while proposal metadata retains the reference. See [Proposals and file history](proposals.md).

## Control Persistence

Durable control state is serialized independently from file CRDT state. Tasks, members, locks, retained chat, and MLS state survive restart. Presence and short-lived activity do not.

Writes use a temporary file and atomic replacement so a partial process failure does not leave the primary control file half-written. See [Storage and persistence](storage.md).

## Peer Sessions

Direct peer sessions pass through these layers:

1. TCP connection;
2. Circle pre-shared-key gate;
3. Noise authentication tied to device identity;
4. Yamux multiplexing;
5. a versioned application stream for content, proposals, events, control state, or awareness.

The rendezvous path supplies peer discovery and forwarding when direct connectivity is unavailable. Application content frames are protected with MLS-derived encryption on both paths. Exact framing is in [P2P protocols](../reference/p2p-protocols.md).

## MLS State

Each Circle persists its MLS group state and current epoch. Membership changes advance the epoch. The daemon derives purpose-specific content keys from the MLS exporter rather than using raw group secrets directly.

Old ciphertext can require an older retained epoch key. Retention and cleanup therefore need to preserve the epochs still referenced by local history.

## API and CLI

The daemon exposes a loopback HTTP API. The CLI translates commands into that API and renders human or JSON output. Mutating routes require the local bearer token.

The API contains compatibility endpoints for pending proposals. They do not imply that the live watcher places ordinary edits into a pending queue.

## Agent Runtime

Agent configuration, sessions, workspace association, process lifecycle, and memory are stored outside the synchronized project files. Agent output still reaches collaboration through native file writes and the watcher pipeline; agent-specific code does not bypass CRDT synchronization.

## Failure Boundaries

- A disconnected peer does not block local edits.
- A relay failure does not corrupt local state; peers can reconnect directly or through another relay.
- A watcher error is isolated to the affected path and should be reported without terminating the Circle.
- Malformed or oversized network frames are rejected before application.
- Durable state should be written atomically and validated during restore.
