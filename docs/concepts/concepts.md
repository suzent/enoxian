# Core Concepts

## Circle

A Circle is a local-first collaboration group with a shared workspace, membership boundary, synchronization secret, and peer-discovery configuration. It has no required central server.

## Device Identity

Each installation owns an Ed25519 signing identity. Its public key is the stable device ID used for membership, attribution, and authenticated peer handshakes.

## Workspace

A workspace is the ordinary directory watched by `enoxd`. Editors and agents access it through native file I/O. enoxian synchronizes what happens there; it does not replace the filesystem.

## Per-file CRDT

Each synchronized path is represented by its own Yjs document. This keeps updates scoped to one file and lets concurrent edits converge independently.

## Proposal Snapshot

A proposal is an immutable version of a file plus its parent, author, origin, timestamp, and status. In the normal live workspace, captured proposals are immediately **accepted** and serve as history for diff, merge, and revert. A **pending** status exists for compatibility and future isolated workflows, not for routine file edits.

## Causal Event

An event records an activity and its causal parents. Event synchronization supplies attributable history without requiring clocks on different machines to agree exactly.

## Control State

Control state is the synchronized circle-level document for tasks, members, locks, retained chat, and security state. It is distinct from file content and from ephemeral awareness.

## Task and Claim

A task is a unit of coordinated work. Claiming it tells other participants who is responsible. Tasks reduce duplicated effort; they do not grant exclusive access to files.

## File Lock

A file lock, exposed as `bind`/`release` in the CLI, is an advisory coordination lease for conflict-prone paths. CRDT synchronization still provides convergence, but the lock communicates that others should wait before editing.

## Presence and Awareness

Presence describes connected participants and transient activity such as a current file. It is intentionally short-lived and is not restored as durable project history.

## Invitation and Membership

An invitation carries the information needed to find and authenticate to a Circle. Membership policy determines whether a joining device is admitted automatically or creates a pending **membership request**. This is separate from file proposal status.

## Agent Session

An agent session associates a configured coding agent with a Circle, workspace, lifecycle state, and durable memory. Agent sessions use the same native files and synchronization path as every other participant.

## MLS Epoch

The Circle's MLS group state advances through epochs as membership changes. Exported epoch secrets derive keys for content-frame encryption and support cryptographic revocation of removed members.

## Relay

A rendezvous relay helps peers connect when direct routing is unavailable. It forwards encrypted traffic and is not authoritative for workspace or control state.

## Bootstrap

Bootstrap is the authenticated initial transfer of a Circle snapshot to a new peer. Continuous synchronization takes over after the snapshot is installed.

## Related Reading

- [How enoxian works](overview.md)
- [Architecture](architecture.md)
- [Proposals and file history](proposals.md)
- [Storage and persistence](storage.md)
- [Security](security.md)
