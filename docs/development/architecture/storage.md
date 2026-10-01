# Storage And Persistence

File IO stays native. enoxian coordinates and records workspace effects but
does not proxy application reads and writes.

## Workspace-Local State

| Path | Purpose |
|------|---------|
| ordinary workspace files | canonical user-visible content |
| `.enox_crdt/` | persisted per-file Yjs state |
| `.enox_proposals/` | proposal records, snapshot manifests, and blobs |
| `.enox_events/events/` | immutable causal workspace events |

These internal directories are excluded from proposal capture and workspace
path APIs.

## Circle-Local State

`~/.enoxian/circles/<circle-id>/` contains device-local circle state:

| Path | Purpose |
|------|---------|
| `config.toml` | circle id/name, workspace, stable PSK, peers and network options |
| `admin.key` | admin signing key, present only where provisioned |
| `control.json` | durable control-document snapshot |
| `mls/` | OpenMLS identity and group storage |
| `agent_sessions/` | best-effort ACP session ids by agent |

`control.json` persists tasks, member records, lock history, retained chat, and
MLS delivery state. Presence and short-lived chat activity are omitted because
they are leases, not durable facts. Chat retention is time-bounded and message
ids make restore idempotent. Writes are debounced and a clean daemon shutdown
also flushes state.

Restored state is seeded into a fresh Yjs document, then merged normally with
peers. A full-circle offline restart therefore retains durable coordination
state without persisting stale presence.

## At-Rest Boundary

All of the above is plaintext on the member device. MLS protects P2P content in
transit and after member removal; it does not encrypt native workspace files or
local stores. Use host full-disk encryption when local filesystem access is in
the threat model.
