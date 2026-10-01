# Workspace Changes And Proposals

enoxian treats the native workspace as canonical. Humans, agents, formatters,
and scripts edit ordinary files with their own tools; they do not need an
enoxian-specific filesystem API or branch directory.

## Capture Flow

```text
native file write
  -> watcher records the pre-change blob
  -> three-second idle window groups the write burst
  -> immutable base and result snapshots
  -> proposal record and causal workspace events
  -> accepted, revertible history
  -> encrypted peer synchronization
```

The proposal store is an audit and undo layer over changes that have already
landed in the live workspace. It is not a staging area.

## When Files Are Pending

Ordinary edits and explicitly requested agent work normally become accepted
history. Writes associated with `AmbientTriggered` sessions, and unmediated
writes captured while ambient review is required, become pending proposals.
These writes have already reached the live workspace: pending status requests
review, rather than providing filesystem isolation.

Use `enox proposal show` to inspect a change, `accept` or `reject` for a pending
review decision, and `revert` to undo accepted history. See the
[user guide](../../concepts/proposals.md) for the workflow.

## Attribution

Attribution describes evidence, not authority:

| Source | Confidence |
|--------|------------|
| `enox agent run` or a pushed ACP/argv mention | managed process/session |
| `enox session start` / `finish` | user-declared time window |
| nearby local activity | inferred |
| ordinary unmatched file write | unknown/ambient |

Missing attribution never discards a change. Chat mentions are ordinary synced
messages expressing intent; only the receiving device's local `agents.toml`
policy can launch a process.

## Storage

`<workspace>/.enox_proposals/` is the durable source of truth:

```text
.enox_proposals/
  proposals/   proposal JSON records
  snapshots/   immutable path -> blob manifest JSON
  blobs/       content-addressed file bytes
  baseline     current baseline snapshot id
```

Proposal records reference immutable base and result snapshots. Large blobs are
not embedded in proposal bundles above 256 KiB; peers request missing blobs by
hash. Reject/revert fails cleanly when required content is unavailable rather
than treating a missing blob as a deletion.

## Diff, Merge, And Revert

Adapters produce useful review output without requiring an agent to emit a
structured patch:

| Content | Review strategy |
|---------|-----------------|
| Text | line hunks |
| Markdown | heading/paragraph changes |
| JSON | object-path changes |
| Common code languages | function/class grouping plus text fallback |
| Binary | hashes and sizes |

Revert uses a three-way reverse apply:

```text
base    = state before the proposal
result  = state recorded by the proposal
current = workspace when revert is requested
```

Disjoint later edits are retained. Overlapping edits produce explicit conflict
paths and a conflict event instead of silently overwriting current work.

## Causal Event Log

Immutable events live at `.enox_events/events/<uuid>.json`. Each contains a
schema version, circle and origin identity, causal parents, Lamport time,
timestamp, and one typed workspace/proposal/merge/conflict payload.

Materialization sorts the event union deterministically and derives the current
snapshot, proposal statuses, completed merges, conflict paths, and causal
frontier. Concurrent decisions use explicit status precedence:

```text
reverted > rejected > accepted/synced > conflicted > pending
```

Older proposal records are backfilled idempotently on upgrade. The event set is
the authority for decisions; mutable proposal records remain as compatibility
views for the API and frontend.
