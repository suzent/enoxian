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

### M14 — Local Workspace Proposals

**Status:** Planned

Add an agent-agnostic proposal layer so local agents, editors, scripts, and
tools can make arbitrary filesystem changes in the normal workspace while
enoxian captures those changes as reviewable proposals.

**MVP flow:**

```text
1. Workspace is clean at snapshot S0
2. Agent/editor/script mutates normal workspace files
3. Watcher captures before blobs for touched paths
4. Idle window closes or session finishes
5. Snapshot result S1
6. Generate S0 -> S1 diffs
7. Create shadow proposal
8. Accept, reject, revert, sync, or mark conflicted
```

**Build order:** the file change layer lands before the trigger layer. A
trigger without the snapshot journal can launch an agent but cannot capture or
review what it did; the ambient proposal flow is useful on its own and
validates the pipeline before remote triggers add complexity.

**Tasks (in order):**

1. Foundation
   - [ ] Content-addressed blob store.
   - [ ] Snapshot manifest format.
   - [ ] Snapshot journal for ambient workspace edits (before-blob capture).
2. Ambient proposal flow
   - [ ] Dirty proposal grouping by idle window/session.
   - [ ] Snapshot diff generation.
   - [ ] Three-way merge against current canonical state.
   - [ ] Proposal records in the control/event layer.
   - [ ] CLI: proposal list/show/accept/reject/revert.
3. Sessions and attribution
   - [ ] Local change session model with attribution confidence.
   - [ ] CLI: session start/finish (claimed session mode).
4. Trigger layer
   - [ ] Circle trigger protocol: `agent_triggered` event schema, replication,
         status replies.
   - [ ] Daemon trigger handler: local allowlist, agent registry config,
         launch-on-trigger.
5. Hardening and UX
   - [ ] Managed process mode (`enox agent run`) with optional sandbox.
   - [ ] Acceptance policy: auto-accept with history for local triggers,
         pending review for remote-member triggers.
   - [ ] Frontend proposal review/history view.
   - [ ] Optional sandbox/manual fork mode for high-risk managed runs.

The proposal watcher is a new layer alongside the existing CRDT sync watcher
(`src/sync_yjs/watcher.rs`), not a replacement: the CRDT watcher keeps serving
interactive editing, while the proposal layer treats the same file events as
session evidence (before-blob capture, idle-window close, S0 -> S1 diff).

### M15 — Event Log And Blob Sync

**Status:** Planned

Move cross-device workspace coordination toward events, snapshot manifests, and
content-addressed blobs instead of raw folder mirroring.

**Tasks:**

- [ ] Event schema for workspace forks, snapshots, proposals, merges, rejects, and conflicts.
- [ ] Content blob request/response protocol over libp2p.
- [~] Missing-blob fetch on proposal receipt. Not needed yet: each proposal
      replicates with its referenced blobs bundled in the control doc
      (`ProposalBundle`), so receipt never leaves a blob missing. A pull-based
      fetch protocol is only required once blobs are decoupled from the bundle
      (e.g. to keep large binaries out of the CRDT).
- [ ] Snapshot materialization from event log.
- [ ] Conflict metadata sync across peers.
- [x] Proposal state replication in the control doc or a dedicated event log.
      Done via the control-doc `proposals` map; see `src/proposal/sync.rs` and
      the observer in `AppState::new`. Create + accept/reject/revert all
      replicate, so every device shows the same review history.

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
