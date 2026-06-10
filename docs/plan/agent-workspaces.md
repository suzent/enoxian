# Local Workspace Proposals

## Summary

The next coordination layer should be agent-agnostic, but it must not require
agents to work in special branch directories by default.

Most local agents are awakened from outside enoxian: a chat mention, an editor
integration, a terminal command, or a separate agent UI. To the agent, the
enoxian workspace is just an ordinary local folder. The default design should
respect that.

Core principle:

```text
Agents do not need to know that enoxian exists.
enoxian observes local filesystem effects and turns them into proposals.
```

Default flow:

```text
normal enoxian workspace
  -> local agent/editor/script mutates files
  -> watcher captures before/after state
  -> enoxian creates a shadow proposal
  -> user or policy accepts, rejects, syncs, or resolves conflicts
```

Sandboxed or forked workspaces are still useful, but they are optional modes for
high-risk or explicitly managed runs.

## Why This Layer Exists

Current Yjs sync is good for interactive editing:

- browser editor
- cursors and selections
- small live text edits
- local user collaboration

Agent-driven file changes are different:

- agents may be triggered by chat mentions
- agents may be launched by external tools
- agents often rewrite whole files
- tools and formatters can touch many paths
- scripts may generate artifacts
- the real editing process is not observable
- different agents have incompatible APIs

For that world, operation-level CRDT truth is the wrong target. The safer
boundary is the filesystem mutation boundary.

## Mental Model

The enoxian workspace is a working tree:

```text
Canonical snapshot S0
  -> ordinary local workspace files
  -> local edits happen
  -> dirty result S1
  -> proposal P1 = S0 -> S1
```

The "branch" is not necessarily a visible directory. In the default mode it is a
shadow branch maintained by enoxian:

```text
agent sees:    ./project
user sees:     ./project
enoxian sees:  base snapshot + dirty working tree + proposal metadata
```

This avoids confusing agents that were asked to work in the real project folder.

## Cases To Support

### 1. Ambient Workspace Mode

This is the default.

An agent, human editor, formatter, or script writes directly to the normal
workspace directory. Enoxian captures the mutation and creates a local proposal.

```text
workspace clean at S0
  -> file write detected
  -> before blob captured for touched path
  -> idle window closes
  -> after snapshot S1
  -> dirty proposal P1
```

Actor attribution may be unknown:

```text
actor_id: null
source: "ambient"
confidence: "unknown"
```

Hints can improve attribution but must not be treated as security facts:

- recent chat trigger
- active local agent session
- terminal/process watcher
- file lock holder
- editor integration
- OS process tree
- recent user selection in the UI

### 2. Chat-Triggered Agent Mode

A user may mention their own or another member's agent in a chat room:

```text
@codex please fix the sync docs
@alice/claude review the proposal layer
```

This creates a trigger event, not a guaranteed process identity.

```text
agent_triggered {
  trigger_id
  circle_id
  requested_agent
  requested_by
  message_id
  workspace_hint
  created_at
}
```

If the local daemon can launch or notify the requested agent, it opens a local
change session:

```text
LocalChangeSession {
  session_id
  trigger_id
  requested_agent
  base_snapshot
  mode: "ambient_triggered"
}
```

The agent still edits the normal workspace unless explicitly sandboxed. When
changes appear near that session, enoxian can attribute the proposal as:

```text
actor_id: "codex"
source: "chat_trigger"
confidence: "session"
trigger_id: "..."
```

If no process binding exists, attribution stays softer:

```text
actor_hint: "codex"
source: "chat_trigger"
confidence: "inferred"
```

The important rule:

```text
Chat mention creates intent.
Filesystem mutation creates the proposal.
```

### 3. Managed Process Mode

Enoxian starts the agent as a child process.

```text
enox agent run --agent codex -- codex .
```

This gives stronger attribution because enoxian controls:

- session ID
- process tree
- start time
- working directory
- base snapshot

The process may still work in the normal workspace:

```text
mode: "managed_process"
workspace: canonical
confidence: "verified_process"
```

Managed process mode can optionally use sandboxing:

```text
enox agent run --sandbox --agent codex -- codex .
```

### 4. Claimed Session Mode

The user declares that a period of local work belongs to an actor, but enoxian
does not launch the process.

```text
enox session start --actor codex
# user launches any tool in the workspace
enox session finish
```

This is useful when an agent must be launched from an external UI but the user
still wants proposals attributed.

```text
source: "claimed_session"
confidence: "user_declared"
```

### 5. Sandboxed Workspace Mode

This is optional and explicit. It is best for risky operations, generated files,
formatters, large rewrites, or agents that are safe to launch under enoxian.

```text
canonical workspace at S0
  -> copy/fork to sandbox workspace
  -> agent edits sandbox files
  -> result snapshot S1
  -> proposal P1
  -> merge into canonical workspace
```

The agent sees a normal folder, but not the canonical folder.

```text
source: "sandbox"
confidence: "verified_workspace"
```

### 6. Manual Fork Mode

The user explicitly creates a working copy and hands it to any tool:

```text
enox workspace fork --actor aider
```

This returns a path. The user may `cd` there and run any tool. This is similar to
sandbox mode, but enoxian does not need to own the process lifecycle.

```text
source: "manual_fork"
confidence: "user_declared"
```

### 7. Unknown Dirty Mode

If the normal workspace changes with no active trigger, session, lock, or
process hint, enoxian still creates a proposal.

```text
actor_id: null
actor_hint: null
source: "ambient"
confidence: "unknown"
```

Do not discard or auto-merge this work just because attribution is missing.
Unknown local edits are normal in an agent-agnostic system.

## Proposal Data Model

```text
LocalChangeSession {
  session_id
  circle_id
  base_snapshot
  mode
  trigger_id?
  requested_agent?
  actor_id?
  actor_hint?
  confidence
  started_at
  finished_at?
}

Snapshot {
  id
  files: path -> {
    hash
    size
    mime
    mode
  }
}

Proposal {
  id
  circle_id
  base_snapshot
  result_snapshot
  changed_paths
  diffs
  status
  source
  actor_id?
  actor_hint?
  confidence
  trigger_id?
  session_id?
}
```

Proposal statuses:

```text
pending
accepted
synced
conflicted
rejected
reverted
```

Attribution confidence:

```text
verified_process
verified_workspace
user_declared
session
inferred
unknown
```

## Capturing Ambient Changes

Ambient mode needs a snapshot journal.

```text
1. Workspace is clean at snapshot S0.
2. Watcher sees the first write.
3. Enoxian captures before blobs for touched paths.
4. More writes arrive during a debounce/idle window.
5. Enoxian captures result snapshot S1.
6. Enoxian creates proposal P1.
```

The idle window should be configurable. Agents often write in bursts:

```text
write file
run formatter
write lock/temp file
rename temp file into place
run tests
touch generated output
```

The proposal should close when:

- the managed process exits
- the claimed session finishes
- a chat-triggered session times out
- the workspace is idle for a configured interval
- the user manually closes the proposal

## Merge And Conflict Model

Agent edits use a Git-like three-way merge:

```text
base   = snapshot when the local change session started
main   = latest accepted canonical snapshot
result = local dirty result
```

Merge outcomes:

```text
clean merge
conflict
stale / rerun recommended
reject
revert local changes
```

This avoids pretending that agent edits are live CRDT operations. They are
commit-level changes.

## Acceptance Policy

Proposals default to auto-accept with full history, similar to git commits:

```text
agent edits files
  -> proposal created (S0 -> S1 diff recorded)
  -> auto-accepted into canonical state
  -> history entry persisted
  -> user can view the diff or revert at any time
```

Blocking on manual review breaks the flow for intentionally triggered agents,
and most runs do not need it. The safety property comes from the audit trail
and revert path, not from a pre-merge gate.

The exception is cross-device triggers. Acceptance defaults by trigger origin:

```text
local agent triggered by local user    -> auto-accept
local agent triggered by remote member -> pending review (configurable)
remote agent on remote device          -> their daemon decides; only status
                                          replies come back
```

Auto-accept is only safe once the undo path is solid. The blob store,
snapshot diff, and revert command must land before auto-accept is enabled by
default.

## Event Model

Workspace proposals should become events:

```text
agent_triggered
local_change_session_started
file_changed
workspace_snapshotted
proposal_created
proposal_accepted
proposal_synced
proposal_rejected
proposal_reverted
proposal_conflicted
```

Peers sync:

```text
event log
snapshot manifests       (implemented — bundled with each proposal)
content-addressed blobs  (implemented — bundled with each proposal)
proposal metadata        (implemented — see below)
```

They do not blindly mirror folders.

**Proposal replication (implemented).** Each proposal is packaged as a
`ProposalBundle` (the proposal record plus its base/result snapshot manifests and
the content blobs those manifests reference) and written into the `__control__`
Yjs map under the `proposals` key. Because the control doc replicates over the
existing CRDT sync path, every device materializes the same proposal into its
local `.enox_proposals/` store and renders the identical review history — diffs
included, even on a device that never observed the original edits. Accept /
reject / revert update the synced record's status so review decisions propagate.
See `src/proposal/sync.rs`.

## Diff And Merge Adapters

The protocol should not require agents to emit structured patches. Instead,
enoxian can extract structure from files after the fact.

Adapters:

| File type | Diff strategy |
|-----------|---------------|
| Plain text | line diff |
| Markdown | heading and paragraph diff |
| JSON/YAML | object-path diff |
| Code | function/class-level diff, then line diff |
| Binary | content hash only |

Adapter interface:

```text
match(path, mime) -> bool
diff(base, result) -> structured_diff
merge(base, main, result) -> merge_result
```

## Interaction With Existing CRDT Sync

This layer does not replace Yjs. It narrows Yjs to the surfaces where it shines:

```text
interactive document editing
awareness
presence-adjacent local UI state
```

Local workspace proposals use:

```text
snapshot
diff
proposal
three-way merge
event log
blob sync
```

During migration, accepted proposals can mutate the canonical workspace, which
the existing watcher/CRDT layer observes. Later, the event/blob layer can become
the primary cross-device file substrate.

## Chat Room Trigger Semantics

A chat mention should not directly mutate files or claim that an agent has done
work. It should create a request that a local daemon may act on.

### Two-Layer Split: Circle Protocol vs Local Daemon

The trigger system is split into two layers with a hard boundary:

```text
circle layer:  agent_triggered event (replicated, signed, auditable)
                        |
                        v
local daemon:  allowlist check -> launch -> LocalChangeSession -> watcher
                        |
                        v
circle layer:  trigger status reply (delivered/started/ignored/expired/completed)
```

The circle protocol only carries intent and status feedback. It never encodes
how a specific agent is launched. All agent-specific logic lives in the daemon:

```text
circle event (portable):        daemon-local (machine-specific):
  requested_agent                 which binary to run
  task_text                       command template
  requested_by                    working directory
  message_id                      session timeout
  workspace_hint                  sandbox policy
```

No webhooks are needed. The replicated control doc / event log is the delivery
channel. This reuses the existing authenticated P2P transport (PSK-gated,
MLS-backed), tolerates offline targets, and adds no new HTTP surface.

### Local Authority

The daemon on the target device is the execution boundary:

```text
1. Check allowlist: is the requested agent permitted on this device?
2. Check requested_by: is this peer a trusted circle member?
3. If yes -> launch agent via the agent registry, open LocalChangeSession.
4. Emit trigger status back to the circle.
```

The allowlist of agents a device will auto-wake lives in local daemon config,
never in synced state, so a remote peer cannot force-enable an agent on
another device. `requested_agent` is a hint for routing, not a security
boundary; the local allowlist is the gate.

### Agent Registry

The daemon maps agent names to launch commands:

```text
[agents.claude]
command = ["claude", "--print", "-p", "{{task}}"]

[agents.codex]
command = ["codex", "{{task}}"]
```

`{{task}}` is the text after the mention. Adding a new agent is a daemon
config change; the circle event schema never changes for it.

Possible trigger outcomes:

```text
delivered     # target device/agent saw it
started       # local session opened
ignored       # no matching agent
expired       # no response before timeout
completed     # proposal created
failed        # launch or runtime error
```

If the target agent belongs to another member, the trigger is replicated as a
circle event. That member's daemon decides whether it can wake the local agent.

This keeps authority local:

```text
remote user can request work
local daemon decides whether and how to run local agents
filesystem changes still become proposals
```

## CLI Sketch

```text
enox session start --actor codex
enox session finish

enox agent run --agent codex -- codex .
enox agent run --sandbox --agent codex -- codex .

enox proposal list
enox proposal show <proposal-id>
enox proposal accept <proposal-id>
enox proposal reject <proposal-id>
enox proposal revert <proposal-id>
```

## Open Questions

- What is the right idle window for ambient sessions?
- How should chat-triggered sessions time out?
- Should proposal events live in the existing control doc or a separate log?
- What is the minimum blob-store format?
- How should proposal review appear in the frontend?
- Which diff adapters should ship first?
- How should direct edits to the canonical workspace be grouped?
- How much process attribution is feasible on Windows, macOS, and Linux?
- What is the right revert granularity (whole proposal vs per-file)?
- Should the pending-review default for remote-member triggers be per-agent or
  per-member?
