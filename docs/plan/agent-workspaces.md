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

A user may mention their own or another member's agent in a chat room. Mentions
address the member hierarchy at three levels (`src/agent/mention.rs`):

```text
@codex please fix the sync docs        # bare agent — any device that
                                        # allowlists `codex` may react
@alice/laptop/claude review the layer   # a specific device's agent — only
                                        # that device reacts
@alice            / @alice/laptop       # a user / a device — notify only,
                                        # launches nothing (for now)
```

Only agent-level targets launch. A device-scoped agent mention runs *only* on
the device whose owner and label match — a device never runs an agent addressed
to a different device. Bare `@agent` keeps the original any-allowlisting-device
behavior. The frontend chat box offers a `@` autocomplete over the
user → device → agent tree, marking which agents are currently reachable
(online + advertised); reachability is a hint, not a guarantee, since a remote
device's push/pull policy is never synced.

This is an ordinary chat message, not a dedicated wire command. It carries
intent, never a guaranteed process identity — and never an instruction that a
remote member can force another device to obey (see
[Two-Layer Split](#two-layer-split-chat-intent-vs-local-reaction)).

Under a **push** reaction policy, the local daemon matches the mention against
its allowlist and opens a local change session:

```text
LocalChangeSession {
  session_id
  requested_agent
  message_id        # the chat message that prompted the run
  base_snapshot
  mode: "ambient_triggered"
}
```

The agent still edits the normal workspace unless explicitly sandboxed. When
changes appear near that session, enoxian can attribute the proposal as:

```text
actor_id: "codex"
source: "chat_mention"
confidence: "session"
message_id: "..."
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

#### Concurrent actors on one device

Claimed sessions attribute by wall-clock window, not by process. This is fine
for a single actor but ambiguous when two agents run on the same device at once:

```text
enox session start --actor codex   # window A
enox session start --actor aider   # window B
  codex writes foo.rs
  aider writes bar.rs
enox session finish                # which actor closed?
```

Both windows cover both files, so enoxian cannot say `foo.rs` was codex and
`bar.rs` was aider. Two windows also make a bare `enox session finish`
ambiguous — it needs an actor or session id.

This is not a plumbing gap; it is a limit of the mode. Claimed sessions
deliberately drop the process binding (that is what makes them `user_declared`
rather than `verified_process`), and without that binding there is nothing to
tie a specific file to a specific concurrent actor. The stronger modes do not
have this problem: `agent run` (managed process) attributes by process tree, and
sandbox/fork give each actor a separate workspace tree.

Candidate resolutions, none yet chosen:

- **Single-actor claimed sessions.** Reject `session start` while another is
  open; document that concurrent agents must use `agent run` or per-agent forks.
  Honest and cheap, but limiting.
- **Path-scoped sessions.** `enox session start --actor X --path sub/dir` claims
  a subtree; overlapping actors are disambiguated by which path they touch.
  Works until two agents edit the same files, then attribution collapses again.
- **Explicit close.** Require `enox session finish --actor X` (or a session id)
  so at least the close is unambiguous, even if per-file attribution stays soft.

See [Open Questions](#open-questions).

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

Long term, peers sync:

```text
event log
snapshot manifests
content-addressed blobs
proposal metadata
```

They do not blindly mirror folders.

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

### Two-Layer Split: Chat Intent vs Local Reaction

The trigger system is split into two layers with a hard boundary:

```text
circle layer:  chat message with @mention (replicated, signed, auditable)
                        |
                        v
local policy:  each device decides how (or whether) to react to the mention
                        |
              push ─────┴───── pull
                |               |
                v               v
        daemon auto-runs   agent proactively
        the local agent    retrieves chat and
        (execution layer)  decides to act
```

**The network side is not a dedicated command; it is chat.** An `@mention`
is an ordinary chat message (M9) that replicates like any other. There is no
imperative "run agent X on device Y" event travelling the wire — a remote
member cannot *cause* execution anywhere. The mention is only intent.

**The reaction is a local policy over the chat stream**, chosen per device:

- **Push** — the daemon subscribes to chat, matches mentions against its local
  allowlist, and auto-launches the agent through the local execution layer.
- **Pull** — the daemon does nothing on its own. The agent proactively reads
  the chat room on its own cadence and decides whether to act. enoxian still
  captures whatever files it changes as proposals.

Both policies converge on the same local execution layer and the same proposal
capture; they differ only in *what initiates the run*. A device may also do
neither (mentions are just messages until someone opts in).

> Note: an earlier prototype materialized a distinct `AgentTriggered` event with
> a `TriggerStatusReply` handshake (`src/trigger/`). That was **removed** — a
> replicated command that lets a remote member push execution at a device is the
> dangerous framing this model exists to avoid. The mention is plain chat; "did
> an agent react?" is optional, local status, not a required network round-trip.
> See roadmap M14.

Whatever initiates a run, the circle layer never encodes how a specific agent
is launched. All agent-specific logic lives in the daemon:

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

Under a **push** reaction policy the daemon, on seeing a mention:

```text
1. Check allowlist: is the mentioned agent permitted on this device?
2. Check the sender: is this a trusted circle member? (affects acceptance policy)
3. If yes -> launch agent via the local execution layer, open LocalChangeSession.
```

The allowlist of agents a device will auto-wake lives in local daemon config,
never in synced state, so a remote peer cannot force-enable an agent on
another device. The mentioned agent name is a routing hint, not a security
boundary; the local allowlist is the gate. Under a **pull** policy there is no
step 3 — the agent itself reads the room and decides.

### Agent Config (Allowlist + Driver)

*To be built with the M14 local reaction layer.* The daemon maps agent names to
launch config; this doubles as the push-policy allowlist:

```text
[agents.claude]
command = ["claude", "--print", "-p", "{{task}}"]   # driver = "argv" (default)

[agents.gemini]
driver = "acp"
command = ["gemini", "--acp"]
```

`{{task}}` is the text after the mention. Adding an agent is a local config
change; nothing on the wire changes for it. (This replaces the removed
`src/trigger/registry.rs`, redesigned to carry a per-agent `driver`.)

If the target agent belongs to another member, the mention still travels as an
ordinary chat message. That member's daemon (under its own push/pull policy)
decides whether it can wake the local agent.

### Local Execution Layer: Raw Argv vs ACP

The registry above launches agents as **fire-and-forget argv**: substitute
`{{task}}`, spawn, and infer the result from the snapshot journal. This is the
universal fallback and must stay the default — it upholds the core principle
that *agents do not need to understand enoxian*.

An optional second driver is the [Agent Client Protocol
(ACP)](https://agentclientprotocol.com/), for agents that speak it (e.g. Zed's
ecosystem, Gemini CLI). Here **enoxian is the ACP client and the coding agent is
the ACP agent**, over JSON-RPC/stdio to a local subprocess. ACP is a *local
execution driver only* — it is not a trigger and not a sync transport:

```text
initiator (push policy, `enox agent run`, or local mention)
   -> local execution layer
        ├── argv driver:  spawn command, infer changes from snapshot journal
        └── acp  driver:  spawn ACP agent, drive prompt turn, mediate fs writes
```

Why ACP is worth having as a driver:

- **Real completion signal.** The ACP prompt-turn lifecycle gives a structured
  start → stop-reason, instead of "the process exited."
- **Strong attribution.** enoxian owns the ACP subprocess and its session, so
  runs are `managed_process` / `verified_process` confidence — and each agent
  gets its own session, which sidesteps the concurrent-actor ambiguity that
  claimed sessions have (see [Claimed Session Mode](#4-claimed-session-mode)).
- **Per-write visibility.** When the agent uses client-provided fs methods
  (`fs/write_text_file`), enoxian sees each write as it happens instead of
  diffing the whole workspace afterward. (An ACP agent that touches disk
  directly falls back to the snapshot-journal path.)
- **Policy hook.** ACP's `session/request_permission` flow is a natural place to
  route a write through the [Acceptance Policy](#acceptance-policy) before it
  becomes canonical — the same "mediate an effect before it lands" idea at a
  finer grain.

Caveats: ACP only covers ACP-speaking agents, so it is one launch mode among
several, never a requirement. Its remote HTTP/WebSocket mode is editor↔agent
remoting and is **not** a substitute for the libp2p circle transport, MLS, or
cross-device proposal replication.

The registry declares a per-agent driver so the initiator stays dumb:

```text
[agents.gemini]
driver = "acp"                 # default: "argv"
command = ["gemini", "--acp"]
```

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
enox agent run --agent gemini            # registry driver = "acp"

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
- How should claimed sessions handle multiple concurrent actors on one device
  (single-actor only, path-scoped sessions, or explicit close)? See
  [Claimed Session Mode](#4-claimed-session-mode).
- Should the network side drop the dedicated `AgentTriggered` event and
  `TriggerStatusReply` handshake in favor of plain chat mentions plus a local
  push/pull reaction policy? See [Two-Layer Split](#two-layer-split-chat-intent-vs-local-reaction).
- For the ACP driver, do we require agents to use client `fs/*` methods (rich
  per-write capture) or also support direct-disk ACP agents via the snapshot
  journal fallback? See [Local Execution Layer](#local-execution-layer-raw-argv-vs-acp).
