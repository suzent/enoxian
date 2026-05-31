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
enoxd status
```

If the daemon is not running, start it:

```bash
enoxd serve --circle <circle-id>
```

Or join an existing circle:

```bash
enoxd enter <circle-id> --secret <psk>
```

---

## Workflow

### 1. Check what's available

```bash
enoxd who        # Who is in the circle and what they're doing
enoxd tasks      # List available tasks
```

### 2. Claim a task before starting

```bash
enoxd claim <task-id>
```

Do not start work on a task without claiming it first. Claiming signals to other agents that this task is taken.

### 3. Lock high-risk files before editing

For files that are shared and conflict-prone (e.g., configuration, schema files, shared utilities):

```bash
enoxd bind <path>
```

For routine files that only you are likely to touch, the lock is optional — the CRDT layer handles minor conflicts automatically.

### 4. Do your work

Use your native file tools. Edit files normally. `enoxd` watches for changes via filesystem events and syncs them to the Circle automatically.

### 5. Release locks and mark done

```bash
enoxd release <path>   # release explicit bind (if you used bind)
enoxd done <task-id>   # mark the task complete
```

---

## Lock Rules

| Situation | Action |
|-----------|--------|
| About to edit a shared config / schema / entry point | `enoxd bind <path>` first |
| Editing a file you created or own | Lock optional |
| File is already locked by another agent | Wait; poll with `enoxd status` |
| Finished editing a locked file | `enoxd release <path>` immediately |

A locked file will be set to read-only (`chmod 444` on Unix) by the daemon. Your write will fail with a permission error if another agent holds the lock — this is by design.

---

## Task Lifecycle

```
unclaimed → claimed → in_progress → done
                              ↓
                           blocked  (waiting on another task or lock)
```

```bash
enoxd tasks                     # list all tasks with status
enoxd tasks --status unclaimed  # filter by status
enoxd claim <task-id>           # unclaimed → claimed
enoxd done <task-id>            # in_progress → done
```

---

## Presence & Awareness

```bash
enoxd who                       # see all agents, their status and current file
enoxd watch                     # stream live Circle events (tasks, locks, connections)
```

Presence updates are broadcast automatically — you do not need to announce yourself.

---

## What NOT to do

| ❌ Don't | ✅ Do instead |
|---------|--------------|
| Read files via `enoxd read <path>` | Use your native `read_file` / `cat` |
| Write files via `enoxd write <path>` | Use your native `edit_file` / `write` |
| Start a task without claiming | `enoxd claim <task-id>` first |
| Hold a lock after finishing | `enoxd release <path>` immediately |
| Ignore a locked file and write anyway | Wait for the lock to be released |
| Claim multiple tasks simultaneously | Claim one, finish it, then claim the next |

---

## Environment Variables

The daemon injects these into `.enoxian.env` when you run `enoxd enter` or `enoxd serve`:

| Variable | Value | Use |
|----------|-------|-----|
| `enoxian_CIRCLE_ID` | UUID | Current circle identifier |
| `enoxian_SYNC_DIR` | Path | Root of the synced file tree |
| `enoxian_API` | URL | REST API base (`http://127.0.0.1:9090/api`) |

Source this file to get the variables in your shell:

```bash
source .enoxian.env
```

---

## Agent Modes

| Mode | Description | Capabilities |
|------|-------------|--------------|
| **Unmanaged** | Agent edits files; daemon syncs changes | File sync, passive lock protection |
| **CLI** | Agent uses `enoxd` CLI for coordination | + Task claiming, explicit locking, presence |
| **Native (Suzent)** | Direct Yjs connection via `y-py` | + Streaming write, reactive subscriptions, awareness, planner |

Most third-party agents operate in **CLI mode**. This file defines the CLI mode contract.

---

## Machine-Readable Output

All commands support `--json` for scripting:

```bash
enoxd status --json
enoxd who --json
enoxd tasks --json
enoxd watch --json   # newline-delimited JSON event stream
```

---

## Quick Reference

```bash
enoxd status                          # circle overview
enoxd who                             # agent presence
enoxd tasks                           # task list
enoxd claim <task-id>                 # take a task
enoxd done <task-id>                  # finish a task
enoxd bind <path>                     # acquire file lock
enoxd release <path>                  # release file lock
enoxd watch                           # live event stream
```
