# How enoxian Works

enoxian gives people and coding agents a shared, local-first workspace. Everyone works through ordinary files while a local daemon synchronizes edits, coordinates tasks and locks, and preserves an attributable history.

## Circles

A **Circle** is the collaboration boundary. It combines:

- a workspace directory;
- a stable circle identity and membership policy;
- a shared synchronization key and peer-discovery configuration;
- synchronized files and control state;
- local metadata used to reconnect after a restart.

Each device has its own cryptographic identity. A device joins through an invitation, proves knowledge of the circle secret, and is admitted according to the circle's membership policy.

## Native File Editing

Editors, agents, and scripts read and write files directly. The daemon watches the workspace and converts changes into per-file Yjs updates. Remote updates are materialized back into normal files.

This boundary is important: enoxian coordinates intent and synchronization, but does not require tools to use a special read/write API.

## Convergence and History

Every synchronized path has a live CRDT document. Concurrent edits converge without treating one machine as the central source of truth.

Alongside live state, enoxian records immutable proposal snapshots and causal events. These provide:

- authorship and origin metadata;
- before/after diffs;
- merge and revert operations;
- a durable explanation of how the current file came to exist.

Ordinary local and remote file changes are accepted immediately. They are not placed in a pending queue. The pending proposal state remains in the data model and API for compatibility and possible isolated-workspace workflows, not as the normal agent-era editing path.

See [Proposals and file history](proposals.md).

## Coordination

The shared control plane includes:

- tasks and claims;
- member records and join requests;
- advisory file locks;
- retained chat messages;
- MLS group state and content-key epochs.

Presence, active-file hints, and short-lived activity are awareness signals rather than durable project records.

The CLI exposes these coordination operations. Agents still use their native file tools for the actual work.

## Connectivity

Peers prefer direct authenticated connections and can use a rendezvous relay when direct routing is unavailable. Peer streams are multiplexed by purpose: workspace synchronization, control synchronization, proposal history, event history, and awareness.

Content frames are encrypted with keys derived from the circle's MLS group state, including when traffic crosses a relay. See [P2P protocols](../reference/p2p-protocols.md) and [Security](security.md).

## Local-First Operation

The local workspace remains usable without a network connection. Changes accumulate locally and synchronize when peers reconnect. Circle configuration, file CRDT state, proposal history, event history, and durable control state survive daemon restarts.

The main limitation is at-rest protection: enoxian's internal state files are currently plaintext and depend on host filesystem security. Transport and synchronized content are encrypted in transit.

## Agent Sessions

Agent sessions add structured metadata around native agent processes: configuration, session identity, workspace association, progress, and durable memory. They do not replace the file watcher or give agents a separate filesystem. A managed or external agent and a human editor therefore participate in the same synchronization and history model.

See [Agent integration](../guide/agents.md).

## Where to Go Next

- [Core concepts](concepts.md) defines the vocabulary.
- [Architecture](architecture.md) describes the runtime components and flows.
- [Storage and persistence](storage.md) describes durable state.
- [CLI reference](../guide/cli.md) lists commands.
- [Daemon API](../reference/api.md) documents integration endpoints.
