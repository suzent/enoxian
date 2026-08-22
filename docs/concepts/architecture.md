# Architecture

enoxian is a local-first collaboration daemon. Editors, coding agents, and scripts keep using ordinary files; `enoxd` observes those files, synchronizes their state, and records enough history to explain or reverse changes.

## System Shape

```text
editors / agents / scripts
          │ native file I/O
          ▼
      workspace watcher
          │
          ├── per-file Yjs documents ── peer synchronization
          ├── proposal snapshots ───── diffs, merges, and reverts
          └── causal event log ─────── attribution and history
          │
          ▼
      local API and CLI
```

The daemon coordinates intent and state; it is not a file proxy. Tools read and write the workspace directly.

## Runtime Components

| Component | Responsibility |
|---|---|
| Lifecycle | Starts the daemon, restores circles, and coordinates shutdown |
| State store | Owns circle configuration and per-file Yjs documents |
| File watcher | Translates local filesystem events into CRDT updates |
| Control store | Persists tasks, membership, locks, retained chat, and MLS state |
| Proposal engine | Captures accepted file snapshots, computes diffs, and supports revert/merge |
| Event log | Records causally ordered file activity and attribution |
| P2P network | Discovers peers and carries synchronized state over authenticated streams |
| Local API | Exposes daemon state and mutations to the CLI and integrations |
| Agent layer | Tracks sessions, workspaces, memory, and native-agent configuration |

## State Surfaces

The system deliberately separates the live document from its durable history:

- **Live workspace state** is a Yjs document per path. Local and remote changes converge there.
- **Proposal history** stores immutable snapshots and metadata used for review, merge, and revert. Normal live-workspace changes are accepted immediately; they are not held pending.
- **The causal event log** records what happened, by whom, and after which prior events.
- **Control state** covers circle-level coordination such as tasks, members, locks, chat, and MLS epochs.

See [Proposals and file history](proposals.md) and [Storage and persistence](storage.md) for the concrete rules.

## Data Flow

### Local file change

1. An editor or agent writes a file normally.
2. The watcher identifies the changed path and origin when available.
3. The daemon updates that path's Yjs document.
4. The proposal engine records an accepted snapshot and the event log records the change.
5. The Yjs update is broadcast to connected peers.

### Remote file change

1. An authenticated peer sends a Yjs update.
2. The local document applies the update.
3. The resulting content is written to the workspace.
4. The watcher suppresses the expected echo so the write is not rebroadcast as a new edit.
5. History and attribution are recorded locally.

### Control change

Tasks, membership, locks, retained chat, and related metadata use their own synchronized control document. They are persisted independently from workspace files so transient presence does not become durable project state.

## Transport and Trust Layers

- Direct peer connections use TCP, a circle pre-shared-key gate, Noise authentication, and Yamux streams.
- Rendezvous relays help peers connect when direct routing is unavailable.
- MLS-derived content keys protect synchronized content across direct and relayed paths.
- Bootstrap transfers use a separate authenticated QUIC protocol.
- The local HTTP API is loopback-only by default and uses bearer authentication for mutating operations.

Wire formats and versioning are documented in [P2P protocols](../reference/p2p-protocols.md). The broader security model is in [Security](security.md).

## Storage Layout

Workspace-visible files remain ordinary project files. enoxian keeps synchronization metadata in the workspace and circle control data under the local enoxian home directory. These files are implementation state, not a second user-facing filesystem.

See [Storage and persistence](storage.md) for paths, retention, and at-rest limitations.

## Local Control Plane

The `enox` CLI is a thin client of the daemon's local API. It is used for coordination—joining circles, claiming tasks, taking locks, inspecting peers, and managing proposals—not for transporting file contents. Native file tools remain the source of workspace edits.
