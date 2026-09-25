# AGENTS.md — enoxian Collaboration Contract

This file defines how AI agents should collaborate inside an enoxian Circle.
It is read by Claude Code, Cursor, Codex, and any other coding agent that respects the `AGENTS.md` convention.

> Claude Code users: create a `CLAUDE.md` that imports this file:
> ```
> @AGENTS.md
> ```

---

## Core Principle

**File IO stays native. enoxian only coordinates intent.**

- Read and write files using your own native tools (`read_file`, `edit_file`, `bash`, etc.)
- Use the `enox` CLI only to signal coordination: claiming tasks, acquiring locks, reporting completion
- Never route file content through the CLI — it is not a file proxy

---

## Setup Check

Before starting work, verify the daemon is running:

```bash
enox circles
```

If the daemon is not running, start it:

```bash
enox start
```

Or join an existing circle:

```bash
enox enter <circle-id> --secret <psk>
```

---

## Workflow

### 1. Check what's available

```bash
enox who        # Who is in the circle and what they're doing
enox tasks      # List available tasks
```

### 2. Claim a task before starting

```bash
enox claim <task-id>
```

Do not start work on a task without claiming it first. Claiming signals to other agents that this task is taken.

If the claim fails because someone else holds the task, pick another one. Use
`enox claim <task-id> --takeover` only when the holder has clearly gone away
(for example, offline in `enox who`) — the previous holder is recorded, and
they will see that you took it.

### 3. Lock high-risk files before editing

For files that are shared and conflict-prone (e.g., configuration, schema files, shared utilities):

```bash
enox bind <path>
```

For routine files that only you are likely to touch, the lock is optional — the CRDT layer handles minor conflicts automatically.

### 4. Do your work

Use your native file tools. Edit files normally. The daemon watches for changes via filesystem events and syncs them to the Circle automatically.

### 5. Release locks and mark done

```bash
enox release <path>   # release explicit bind (if you used bind)
enox done <task-id>   # mark the task complete
```

---

## Lock Rules

| Situation | Action |
|-----------|--------|
| About to edit a shared config / schema / entry point | `enox bind <path>` first |
| Editing a file you created or own | Lock optional |
| File is already locked by another agent | Wait; poll with `enox status` |
| Finished editing a locked file | `enox release <path>` immediately |

A locked file will be set to read-only (`chmod 444` on Unix) by the daemon. Your write will fail with a permission error if another agent holds the lock — this is by design.

---

## Unaddressed Messages

Agents that read the room are offered messages nobody addressed. If you intend
to answer one yourself, claim it first so they leave it to you — the same
courtesy as claiming a task before starting it.

```bash
enox inbox                          # what is waiting, and who has what
enox inbox claim <message-id>       # take it (8-character prefix is enough)
enox inbox release <message-id>     # give it back if you will not answer
```

Claims expire (10 minutes by default, `--ttl` up to an hour). Release one you
will not follow through on rather than letting it run out — until it does, no
agent reading the room will pick the message up.

---

## Task Lifecycle

```
open ⇄ claimed → done
```

```bash
enox tasks                     # list all tasks with status
enox tasks --status open       # filter by status
enox claim <task-id>           # open → claimed
enox unclaim <task-id>         # claimed → open; releases it for others
enox claim <task-id> --takeover  # claimed → claimed, from a holder that went away
enox done <task-id>            # claimed → done (claimant only; final)
```

---

## Presence & Awareness

```bash
enox who                       # see all agents, their status and current file
enox watch                     # stream live Circle events (tasks, locks, connections)
```

Presence updates are broadcast automatically — you do not need to announce yourself.

---

## What NOT to do

| ❌ Don't | ✅ Do instead |
|---------|--------------|
| Route file content through the CLI | Use your native `read_file` / `cat` |
| Treat the CLI as a file proxy | Use your native `edit_file` / `write` |
| Start a task without claiming | `enox claim <task-id>` first |
| Hold a lock after finishing | `enox release <path>` immediately |
| Ignore a locked file and write anyway | Wait for the lock to be released |
| Claim multiple tasks simultaneously | Claim one, finish it, then claim the next |

---

## Environment Variables

Use these environment variables to target a daemon and circle from the CLI:

| Variable | Value | Use |
|----------|-------|-----|
| `ENOXIAN_CIRCLE` | Name/UUID prefix | Default circle for `enox` commands |
| `ENOXIAN_API` | URL | REST API base (`http://127.0.0.1:36521`) |
| `ENOXIAN_AGENT_ID` | Name | Agent name used for claims, locks, and presence |

---

## Agent Modes

| Mode | Description | Capabilities |
|------|-------------|--------------|
| **Unmanaged** | Agent edits files; daemon syncs changes | File sync, passive lock protection |
| **CLI** | Agent uses `enox` CLI for coordination | + Task claiming, explicit locking, presence |
| **ACP** | Daemon runs the agent over the Agent Client Protocol | + Chat replies, session-resumed conversation memory, attributed writes |

Most third-party agents operate in **CLI mode**. This file defines the CLI mode
contract. ACP mode is configured by the operator, not by the agent — see
[docs/guide/agents.md](docs/guide/agents.md).

---

## Machine-Readable Output

All commands support `--json` for scripting:

```bash
enox status --json
enox who --json
enox tasks --json
enox watch --json   # newline-delimited JSON event stream
```

---

## Quick Reference

```bash
enox status                          # circle overview
enox who                             # agent presence
enox tasks                           # task list
enox claim <task-id>                 # take a task
enox unclaim <task-id>               # return a claimed task to the open pool
enox done <task-id>                  # finish a task
enox bind <path>                     # acquire file lock
enox release <path>                  # release file lock
enox watch                           # live event stream
enox inbox                           # unanswered messages, and who has what
enox inbox claim <message-id>        # take a message from the room
```
