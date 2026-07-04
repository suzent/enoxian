# enoxian Roadmap

This roadmap tracks current and upcoming work. Completed milestones are archived
under [archived/](archived/).

## Current Direction

enoxian is moving from a real-time file collaboration daemon toward an
agent-agnostic workspace coordination protocol.

The current implementation already has:

- circle creation and invite links
- stable device-derived peer identity
- stable per-circle PSK transport gate
- libp2p P2P sync with relay/rendezvous fallback
- Yjs-based text sync for interactive local editing
- presence, tasks, locks, chat, members, and frontend UI
- MLS membership state and `mls_removed` tombstone sync gate
- the local workspace proposal layer (M14): ambient capture, review, and
  cross-device replication
- agent execution over ACP: chat-mention reactions, `enox agent run`, session
  memory, world-context injection, and CLI/frontend agent config
  (see [../agents.md](../agents.md))

The next design layer is:

```text
normal workspace
  -> local agent/editor/script mutates files
  -> snapshot journal captures before/after
  -> shadow proposal
  -> accept / reject / conflict / sync
```

The key principle is:

```text
Agents do not need to understand enoxian.
enoxian only needs to capture their filesystem effects.
```

See [agent-workspaces.md](agent-workspaces.md) for the proposed design.

---

## Architecture Principles

### Transport Is Not Trust

Bootstrap and relay nodes are centralized network infrastructure, not a
centralized trust core. They can see metadata such as peer IDs, timing, and
traffic volume, but they do not join circles and do not hold circle PSKs.

### Stable PSK, Separate Membership

The transport PSK is a stable per-circle network gate. Member removal is
enforced by the signed member list and the `mls_removed` tombstone sync gate.
Future content encryption will use MLS-derived keys at the message layer, not as
transport PSKs.

### Two Sync Surfaces

enoxian should keep two distinct editing surfaces:

| Surface | Use case | Mechanism |
|---------|----------|-----------|
| Interactive | Browser editor, cursors, small live edits | Yjs / awareness |
| Local workspace proposals | AI agents, scripts, batch edits, local tools | Snapshot journal + diff + proposal |

CRDTs are excellent for live text collaboration. They are not the right default
for arbitrary agent-driven file mutations, formatter rewrites, generated
artifacts, or binary files.

### Canonical State Machine

Long term, the synced object should be:

```text
event log + snapshot manifests + content-addressed blobs + proposal metadata
```

Peers materialize the current workspace from events and snapshots. Relays only
forward encrypted events and blob chunks.

---

## Next Milestones

### M13 — Local API Hardening

**Status:** Planned

The local HTTP/WebSocket API is a privileged control plane for CLI and browser
clients. It should not be treated as a public relay endpoint.

**Tasks:**

- [ ] Default `enoxd` HTTP/WS listener to loopback.
- [ ] Add an explicit flag for LAN/public binding.
- [ ] Replace permissive CORS with a local origin allowlist.
- [ ] Add local API authentication for CLI and browser clients.
- [ ] Document safe remote access patterns.

### M14 — Local Workspace Proposals And Agent Execution

**Status:** Complete (core) — archived. The proposal layer and the full ACP
agent-execution stack (mentions, `enox agent run`, session memory, world
context, agent config, replay-safety) are built and verified against real ACP
agents. Only an *optional* sandbox/fork mode is deferred.

- Milestone record: [archived/milestones.md](archived/milestones.md) → M14
- User guide: [../agents.md](../agents.md)
- Design: [agent-workspaces.md](agent-workspaces.md)

### M14.5 — Control-Doc Persistence

**Status:** Design done, implementation pending. Surfaced during M14 agent work,
not in the original plan. See [control-persistence.md](control-persistence.md).

The `__control__` CRDT (chat, tasks, members, presence) is **in-memory only** —
if every member is offline and a daemon restarts, that circle's chat/task/member
history is lost, because nothing persisted it and no peer remains to re-sync
from. Files are persisted; coordination state is not. This is a correctness gap,
not a feature.

**Tasks (Tier A — selective durability):**

- [ ] Persist tasks and member list to disk; restore before the swarm connects.
- [ ] Persist chat, time-boxed by a retention window (never unbounded).
- [ ] Never persist presence (stale-on-restore is wrong).
- [ ] Reconcile with the mention-replay guards (`handled.rs`) so restored chat
      never re-triggers agents.
- [ ] Document all-offline recovery + plaintext-at-rest (pre-M17) in
      `security.md` / `architecture.md`.

**Deferred (Tier B):** a per-member delivery/read cursor for unread indicators
and delivery-based pruning — no artifact carries a read signal today. Designed
alongside M17 content encryption, not before.

**Open product decisions before implementing:** chat retention window; whether
plaintext chat-at-rest is acceptable before M17. See the design doc's §8.

### M15 — Event Log And Blob Sync

**Status:** Planned

Move cross-device workspace coordination toward events, snapshot manifests, and
content-addressed blobs instead of raw folder mirroring.

**Tasks:**

- [ ] Event schema for workspace forks, snapshots, proposals, merges, rejects, and conflicts.
- [ ] Content blob request/response protocol over libp2p.
- [ ] Missing-blob fetch on proposal receipt.
- [ ] Snapshot materialization from event log.
- [ ] Conflict metadata sync across peers.
- [ ] Proposal state replication in the control doc or a dedicated event log.

### M16 — Diff And Merge Adapters

**Status:** Planned

Make proposal diffs document-aware without requiring agents to produce
structured patches.

**Adapters:**

- [ ] Text line diff.
- [ ] Markdown heading/paragraph diff.
- [ ] JSON/YAML object-path diff.
- [ ] Code-aware diff for function/class-level changes.
- [ ] Binary/hash-only diff.
- [ ] Formatter-noise detection.

### M17 — Layer 4 Content Encryption

**Status:** Planned

Encrypt CRDT updates, event log entries, proposal metadata, and blob chunks with
MLS-derived content keys. This provides cryptographic future secrecy after
member removal while keeping transport connectivity decoupled from membership.

**Tasks:**

- [ ] Define encrypted frame format.
- [ ] Derive content keys from MLS epoch state.
- [ ] Encrypt/decrypt P2P sync payloads.
- [ ] Encrypt proposal events and blobs.
- [ ] Handle epoch changes and offline members.
- [ ] Document residual metadata leakage.

### M18 — Packaging And Distribution

**Status:** Planned

Ship `enoxd` and `enox` as ready-to-use binaries for major platforms.

**Tasks:**

- [ ] GitHub Actions CI across Linux, macOS, and Windows.
- [ ] Release workflow for tagged builds.
- [ ] macOS universal binary and archive.
- [ ] Linux static or portable binaries.
- [ ] Windows zip or installer.
- [ ] Docker image for bootstrap/relay nodes.
- [ ] Quick-install scripts.
- [ ] Homebrew formula.

---

## Reference

- Current security model: [../security.md](../security.md)
- System architecture: [../architecture.md](../architecture.md)
- Completed milestones: [archived/milestones.md](archived/milestones.md)
- Agent workspace proposal design: [agent-workspaces.md](agent-workspaces.md)
