# AGENTS.md — ENOCHIAN Collaboration Contract

This file defines how AI agents should collaborate inside an ENOCHIAN Circle.
It is read by Claude Code, Cursor, Codex, and any other coding agent that respects the `AGENTS.md` convention.

> Claude Code users: create a `CLAUDE.md` that imports this file:
> ```
> @AGENTS.md
> ```

---

## Core Principle

**File IO stays native. ENOCHIAN only coordinates intent.**

- Read and write files using your own native tools (`read_file`, `edit_file`, `bash`, etc.)
- Use `enochd` CLI only to signal coordination: claiming tasks, acquiring locks, reporting completion
- Never route file content through the CLI — it is not a file proxy

---

## Setup Check

Before starting work, verify the daemon is running:

```bash
enochd status
```

If the daemon is not running, start it:

```bash
enochd serve --circle <circle-id>
```

Or join an existing circle:

```bash
enochd enter <circle-id> --secret <psk>
```

---

## Workflow

### 1. Check what's available

```bash
enochd who        # Who is in the circle and what they're doing
enochd tasks      # List available tasks
```

### 2. Claim a task before starting

```bash
enochd claim <task-id>
```

Do not start work on a task without claiming it first. Claiming signals to other agents that this task is taken.

### 3. Lock high-risk files before editing

For files that are shared and conflict-prone (e.g., configuration, schema files, shared utilities):

```bash
enochd bind <path>
```

For routine files that only you are likely to touch, the lock is optional — the CRDT layer handles minor conflicts automatically.

### 4. Do your work

Use your native file tools. Edit files normally. `enochd` watches for changes via filesystem events and syncs them to the Circle automatically.

### 5. Release locks and mark done

```bash
enochd release <path>   # release explicit bind (if you used bind)
enochd done <task-id>   # mark the task complete
```

---

## Lock Rules

| Situation | Action |
|-----------|--------|
| About to edit a shared config / schema / entry point | `enochd bind <path>` first |
| Editing a file you created or own | Lock optional |
| File is already locked by another agent | Wait; poll with `enochd status` |
| Finished editing a locked file | `enochd release <path>` immediately |

A locked file will be set to read-only (`chmod 444` on Unix) by the daemon. Your write will fail with a permission error if another agent holds the lock — this is by design.

---

## Task Lifecycle

```
unclaimed → claimed → in_progress → done
                              ↓
                           blocked  (waiting on another task or lock)
```

```bash
enochd tasks                     # list all tasks with status
enochd tasks --status unclaimed  # filter by status
enochd claim <task-id>           # unclaimed → claimed
enochd done <task-id>            # in_progress → done
```

---

## Presence & Awareness

```bash
enochd who                       # see all agents, their status and current file
enochd watch                     # stream live Circle events (tasks, locks, connections)
```

Presence updates are broadcast automatically — you do not need to announce yourself.

---

## What NOT to do

| ❌ Don't | ✅ Do instead |
|---------|--------------|
| Read files via `enochd read <path>` | Use your native `read_file` / `cat` |
| Write files via `enochd write <path>` | Use your native `edit_file` / `write` |
| Start a task without claiming | `enochd claim <task-id>` first |
| Hold a lock after finishing | `enochd release <path>` immediately |
| Ignore a locked file and write anyway | Wait for the lock to be released |
| Claim multiple tasks simultaneously | Claim one, finish it, then claim the next |

---

## Environment Variables

The daemon injects these into `.enochian.env` when you run `enochd enter` or `enochd serve`:

| Variable | Value | Use |
|----------|-------|-----|
| `ENOCHIAN_CIRCLE_ID` | UUID | Current circle identifier |
| `ENOCHIAN_SYNC_DIR` | Path | Root of the synced file tree |
| `ENOCHIAN_API` | URL | REST API base (`http://127.0.0.1:9090/api`) |

Source this file to get the variables in your shell:

```bash
source .enochian.env
```

---

## Agent Modes

| Mode | Description | Capabilities |
|------|-------------|--------------|
| **Unmanaged** | Agent edits files; daemon syncs changes | File sync, passive lock protection |
| **CLI** | Agent uses `enochd` CLI for coordination | + Task claiming, explicit locking, presence |
| **Native (Suzent)** | Direct Yjs connection via `y-py` | + Streaming write, reactive subscriptions, awareness, planner |

Most third-party agents operate in **CLI mode**. This file defines the CLI mode contract.

---

## Machine-Readable Output

All commands support `--json` for scripting:

```bash
enochd status --json
enochd who --json
enochd tasks --json
enochd watch --json   # newline-delimited JSON event stream
```

---

## Quick Reference

```bash
enochd status                          # circle overview
enochd who                             # agent presence
enochd tasks                           # task list
enochd claim <task-id>                 # take a task
enochd done <task-id>                  # finish a task
enochd bind <path>                     # acquire file lock
enochd release <path>                  # release file lock
enochd watch                           # live event stream
```
