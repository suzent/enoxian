# Core Concepts

## Circle

A **Circle** is the top-level unit of collaboration. It is identified by a UUID and has:

| Property | Description |
|----------|-------------|
| `circle_id` | UUID v4, generated at `enoch init` |
| `circle_name` | Human-readable label |
| `secret` (PSK) | 256-bit pre-shared key; shared out-of-band with new members |
| `sync_dir` | Local directory that is watched and synced (default: `~/.enochian/circles/<id>/files`) |
| Control Doc | In-memory Yjs document holding tasks, presence, and the lock log |

Circles are created with `enoch init` and joined with `enoch enter`. Every participant runs their own `enochd` daemon, which connects to other daemons over libp2p.

---

## Agent

An **Agent** is any process — AI model, script, or human via CLI — that interacts with a daemon. Agents identify themselves by a free-form `agent_id` string passed in request bodies (e.g. `"agent-alpha"`, `"gpt-4o-worker-3"`).

Agents do not need a persistent identity. The `enoch enter` command generates an ephemeral libp2p keypair on each run.

---

## Document

Every file in the sync directory corresponds to a **Yjs Doc** keyed by its relative path (forward-slash normalized). Each Doc holds a single `Y.Text` named after the file path.

Docs are created on first access (`get_or_create_doc`) and live in memory for the lifetime of the daemon. Changes flow both ways:

```
disk file  ←──────────────────────→  Y.Text CRDT
           watcher reads file         WebSocket / P2P update
           updates Y.Text             flushes Y.Text to disk
```

Because Yjs is a CRDT, concurrent edits from multiple agents merge automatically without conflicts.

---

## Control Document

A special in-memory Yjs Doc (not a file) stores circle-wide coordination state under three namespaced Y-collections:

| Key | Yjs type | Holds |
|-----|----------|-------|
| `tasks` | `Y.Map` | Task records, keyed by task UUID |
| `presence` | `Y.Map` | Agent heartbeat records, keyed by agent ID |
| `lock_log` | `Y.Array` | Append-only lock event entries |

All REST API mutations (create task, claim, bind/release) write into the Control Doc via `transact_mut()`. Because it is a Yjs CRDT, future P2P gossip of the Control Doc will merge correctly across nodes.

---

## Lock

A **lock** is a cooperative advisory lock on a relative file path. It is backed by two mechanisms:

1. **Logical lock** — an entry in the `lock_log` Y.Array. The current holder is computed by replaying the log (see [internals.md](internals.md#lock-arbitration)).
2. **Physical lock** — the file's permissions are set read-only (`chmod 444` on Unix, `FILE_ATTRIBUTE_READONLY` on Windows) to prevent accidental overwrites by tools that don't know about ENOCHIAN.

Locks are released either explicitly (via `POST /api/release`) or by re-running the arbitration log if the holder is known to have disconnected.

---

## Event

The daemon emits **Circle Events** over an SSE stream (`GET /api/events`). Every mutation produces at least one event. Events are broadcast in-process via a `tokio::sync::broadcast` channel with capacity 256.

See [protocol.md](protocol.md#sse-event-stream) for the full event type list.
