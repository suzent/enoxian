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
- Use `enoxd` CLI only to signal coordination: claiming tasks, acquiring locks, reporting completion
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

### 3. Lock high-risk files before editing

For files that are shared and conflict-prone (e.g., configuration, schema files, shared utilities):

```bash
enox bind <path>
```

For routine files that only you are likely to touch, the lock is optional — the CRDT layer handles minor conflicts automatically.

### 4. Do your work

Use your native file tools. Edit files normally. `enoxd` watches for changes via filesystem events and syncs them to the Circle automatically.

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

## Task Lifecycle

```
unclaimed → claimed → in_progress → done
                              ↓
                           blocked  (waiting on another task or lock)
```

```bash
enox tasks                     # list all tasks with status
enox tasks --status unclaimed  # filter by status
enox claim <task-id>           # unclaimed → claimed
enox done <task-id>            # in_progress → done
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
| Read files via `enox read <path>` | Use your native `read_file` / `cat` |
| Write files via `enox write <path>` | Use your native `edit_file` / `write` |
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
| **Native (Suzent)** | Direct Yjs connection via `y-py` | + Streaming write, reactive subscriptions, awareness, planner |

Most third-party agents operate in **CLI mode**. This file defines the CLI mode contract.

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
enox done <task-id>                  # finish a task
enox bind <path>                     # acquire file lock
enox release <path>                  # release file lock
enox watch                           # live event stream
```
