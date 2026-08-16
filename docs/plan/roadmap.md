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
- the proposal pull protocol: peer-to-peer anti-entropy for proposal records and
  missing proposal blobs, replacing the old fully replicated control-doc
  proposal map
- agent execution over ACP: chat-mention reactions, `enox agent run`, session
  memory, world-context injection, and CLI/frontend agent config
  (see [../agents.md](../guide/agents.md))

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

**Status:** Complete. The local HTTP/WS API is a privileged control plane and is
now guarded accordingly. See [../reference/daemon.md](../reference/daemon.md) →
Local API security.

**Tasks:**

- [x] Default `enoxd` HTTP/WS listener to loopback. (`src/commands/serve.rs`)
- [x] Add explicit flags for LAN/public binding (`--bind-lan`, `--bind <ip>`).
- [x] Replace permissive CORS with a local origin allowlist. (`src/api/mod.rs`)
- [x] Local API token auth for CLI and browser clients. (`src/api/auth.rs`;
      CLI sends `Authorization: Bearer`, frontend gets the token injected into
      its HTML, WS/SSE use `?token=`)
- [x] Document safe remote access patterns (SSH tunnel loopback).

### M14 — Local Workspace Proposals And Agent Execution

**Status:** Complete (core) — archived. The proposal layer and the full ACP
agent-execution stack (mentions, `enox agent run`, session memory, world
context, agent config, replay-safety) are built and verified against real ACP
agents. Only an *optional* sandbox/fork mode is deferred.

- Milestone record: [archived/milestones.md](archived/milestones.md) → M14
- User guide: [../agents.md](../guide/agents.md)
- Design: [agent-workspaces.md](agent-workspaces.md)

### M14.5 — Control-Doc Persistence

**Status:** Complete (Tier A). The durable control-doc state now survives an
all-offline restart. See [control-persistence.md](control-persistence.md).

Previously the `__control__` CRDT (chat, tasks, members, presence) was in-memory
only — an all-offline restart lost it. Now the durable subset persists to
`<circle_dir>/control.json` and restores before the swarm connects.

**Tasks (Tier A — selective durability):**

- [x] Persist tasks and member list to disk; restore before the swarm connects.
      (`src/store/control.rs`, wired in `lifecycle.rs`)
- [x] Persist chat, time-boxed to 30 days (never unbounded).
- [x] Never persist presence (stale-on-restore is wrong) or MLS scratch.
- [x] Reconcile with the mention-replay guards: restored chat keeps its old
      timestamps, so the reaction loop's `ts` cutoff skips it — a restored
      mention never re-triggers an agent.
- [x] Verified live: post chat+task, hard-kill (no peer, no clean shutdown),
      restart → both restored from disk.
- Product decisions taken: 30-day time-window retention; plaintext at rest is
  acceptable pre-M17 (documented in `concepts/security.md`).

**Deferred (Tier B):** a per-member delivery/read cursor for unread indicators
and delivery-based pruning — no artifact carries a read signal today. Designed
alongside M17 content encryption (and the agent chat-inbox, see
[agent-memory.md](agent-memory.md)), not before.

### M15 — Event Log And Blob Sync

**Status:** In progress. The proposal anti-entropy/blob slice is complete:
`/enoxian/proposals/1.0.0` now reconciles on-disk proposal stores on each peer
connection and transfers only missing or status-diverged proposal bundles
(`src/network/proposal_sync.rs`). Proposal status conflicts converge via the
explicit `(status_rank, updated_at)` rule in `src/proposal/model.rs`, and
proposal data no longer rides the fully replicated control doc. After proposal
manifests are applied, peers request any missing content-addressed blobs so
large proposal files can become reviewable and revertible away from the origin
device.

The remaining work is the broader event-log layer: an event schema for workspace
state, snapshot materialization, and conflict metadata. That layer is still
design-pending and should be built carefully with multi-peer tests.

Move cross-device workspace coordination toward events, snapshot manifests, and
content-addressed blobs instead of raw folder mirroring.

**Tasks:**

- [x] Proposal pull protocol over libp2p for missing proposal bundles and status
      divergence. (`src/network/proposal_sync.rs`;
      [proposal-pull-protocol.md](proposal-pull-protocol.md))
- [x] Content blob request/response round over the proposal protocol for blobs
      that are not embedded in a proposal bundle. (`src/network/proposal_sync.rs`)
- [x] Missing-blob fetch after proposal receipt, so large proposal files can be
      rendered/rejected/reverted once a peer with the content is online.
- [x] Remove proposal replication from the control doc; keep the on-disk
      proposal store as the source of truth. (`src/proposal/sync.rs`,
      `src/state.rs`)
- [x] Deterministic proposal status conflict rule using
      `(status_rank, updated_at)`. (`src/proposal/model.rs`)
- [ ] Event schema for workspace forks, snapshots, proposals, merges, rejects, and conflicts.
- [ ] Snapshot materialization from event log.
- [ ] Conflict metadata sync across peers.
- [ ] Promote proposal status changes into the dedicated event log once that log
      exists, replacing the connection-time-only reconciliation model.

### M16 — Diff And Merge Adapters

**Status:** Complete. Proposal diffs are document-aware without agents producing
structured patches. `src/proposal/adapters/` dispatches by file type/content and
the structured diff is surfaced in the proposal detail API (`FileDiff.diff`).

**Adapters:**

- [x] Text line diff. (`adapters/text.rs`, via diffy)
- [x] Markdown heading/paragraph diff. (`adapters/markdown.rs`)
- [x] JSON object-path diff. (`adapters/json.rs`; YAML falls back to text — no
      YAML parser dependency yet)
- [x] Code-aware diff for function/class-level changes. (`adapters/code.rs`,
      heuristic across common languages, not a full parser)
- [x] Binary/hash-only diff. (`adapters/binary.rs`)
- [x] Formatter-noise detection. (`formatting_only` on every adapter)

### M17 — Layer 4 Content Encryption

**Status:** Planned — design-pending, deliberately not rushed. This is the
highest-stakes cryptographic work in the project: a subtle bug silently breaks
the security guarantees everything else assumes. It must be designed and
reviewed carefully, not fast-drafted, and lands after M15 (it encrypts the event
log / blobs that M15 introduces). Note: control-doc chat is currently persisted
**plaintext** at rest (M14.5) pending this milestone — see
[../concepts/security.md](../concepts/security.md) → Data At Rest.

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

**Status:** Complete (pending live validation). CI, cross-platform release
binaries, a bootstrap Docker image, install scripts, and a Homebrew formula are all
in place. The current release workflow validates gates before publishing and can
update a separately configured Homebrew tap after all assets are available.

**Tasks:**

- [x] GitHub Actions CI across Linux, macOS, and Windows. (`.github/workflows/ci.yml`)
- [x] Release workflow for tagged builds. (`.github/workflows/release.yml`)
- [x] macOS binaries (aarch64 + x86_64 archives). (universal-lipo bundle: future)
- [x] Linux static/portable binaries (musl x86_64 + aarch64).
- [x] Windows zip. (installer: future)
- [x] Docker image for bootstrap/relay nodes. (`Dockerfile`)
- [x] Quick-install scripts. (`scripts/install.sh`, `scripts/install.ps1`)
- [x] Homebrew formula. (`Formula/enoxian.rb`; the release workflow updates SHAs in an optional tap)

---

## Reference

- Current security model: [../security.md](../concepts/security.md)
- System architecture: [../architecture.md](../concepts/architecture.md)
- Completed milestones: [archived/milestones.md](archived/milestones.md)
- Agent workspace proposal design: [agent-workspaces.md](agent-workspaces.md)
