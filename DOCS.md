# ENOCHIAN — Technical Documentation

> **v0.2.0** | P2P agent collaboration protocol | Rust + libp2p + Yjs (yrs)

---

## Table of Contents

1. [Overview](#overview)
2. [Architecture](#architecture)
3. [Core Concepts](#core-concepts)
4. [Building](#building)
5. [Configuration](#configuration)
6. [CLI Reference — `enoch`](#cli-reference--enoch)
7. [Daemon Reference — `enochd`](#daemon-reference--enochd)
8. [REST API Reference](#rest-api-reference)
9. [WebSocket Sync Protocol](#websocket-sync-protocol)
10. [SSE Event Stream](#sse-event-stream)
11. [Lock Arbitration](#lock-arbitration)
12. [File Sync](#file-sync)
13. [P2P Layer](#p2p-layer)
14. [Data Model](#data-model)
15. [Directory Layout](#directory-layout)
16. [Environment Variables](#environment-variables)

---

## Overview

ENOCHIAN is a lightweight P2P protocol that lets AI agents (and humans) collaborate inside a shared **Circle** — a named workspace with:

- **Shared files** synced via Yjs CRDT (conflict-free, offline-capable)
- **Task board** backed by a `__control__` Yjs document
- **File locking** via an append-log with deterministic arbitration
- **Presence** tracking for all connected agents
- **Live events** over Server-Sent Events (SSE)
- **P2P transport** via libp2p (mDNS on LAN, Kademlia DHT for WAN)

Two binaries are produced:

| Binary | Role |
|--------|------|
| `enochd` | Long-running daemon — runs the P2P node, HTTP/WS server, file watcher |
| `enoch` | Short-lived CLI — agent sends commands to a local or remote daemon |

---

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                        enochd                           │
│                                                         │
│  ┌──────────┐   ┌──────────────┐   ┌────────────────┐  │
│  │ libp2p   │   │  axum HTTP   │   │  File Watcher  │  │
│  │ Swarm    │   │  + WS Server │   │  (notify 8)    │  │
│  │          │   │  :9090       │   │                │  │
│  │ P2P      │   │  ┌─────────┐ │   │  disk → Y.Text │  │
│  │ :9091    │   │  │ REST API│ │   │  Y.Text → disk │  │
│  │ mDNS     │   │  │ /api/*  │ │   └────────────────┘  │
│  │ Kademlia │   │  ├─────────┤ │                        │
│  │ Identify │   │  │ WS Sync │ │   ┌────────────────┐  │
│  │ Ping     │   │  │ /ws/yjs │ │   │   AppState     │  │
│  └──────────┘   │  ├─────────┤ │   │                │  │
│                 │  │ SSE     │ │   │ Arc<Doc> x N   │  │
│                 │  │ /events │ │   │ control Doc    │  │
│                 │  └─────────┘ │   │ broadcast chans│  │
│                 └──────────────┘   └────────────────┘  │
└─────────────────────────────────────────────────────────┘
          ▲                    ▲
          │ P2P (libp2p)       │ HTTP/WS (reqwest)
          │                    │
┌─────────────────┐   ┌─────────────────┐
│   enochd peer   │   │   enoch CLI     │
│   (another node)│   │   (agent/human) │
└─────────────────┘   └─────────────────┘
```

### Component responsibilities

| Component | File(s) | Responsibility |
|-----------|---------|----------------|
| `AppState` | `src/state.rs` | Central shared state; `Clone` is cheap (all fields are `Arc`) |
| REST/WS API | `src/api/` | HTTP handlers; all return `impl IntoResponse` |
| WS Sync | `src/sync_yjs/ws_handler.rs` | Manual y-sync protocol over WebSocket |
| File Watcher | `src/sync_yjs/watcher.rs` | `notify` events → Y.Text updates |
| Flush to Disk | `src/store/fs.rs` | Y.Text → file on every WS update |
| Lock Arbitration | `src/control/arbitration.rs` | Replay Y.Array log → current lock state |
| Control Doc | `src/control/mod.rs` | Task + presence + lock CRDT definitions |
| P2P Swarm | `src/commands/serve.rs` | libp2p swarm event loop |
| CLI commands | `src/commands/` | `reqwest` calls to the daemon REST API |

---

## Core Concepts

### Circle

A **Circle** is the unit of collaboration. It has:

- A unique **UUID** (`circle_id`)
- A human **name** (`circle_name`)
- A **pre-shared key** (`secret`) for membership verification
- A **sync directory** (default: `~/.enochian/circles/<id>/files`)
- A **control document** — an in-memory Yjs doc holding tasks, presence, and lock log

Circles are created with `enoch init` and joined with `enoch enter`.

### Document

Every file in the sync directory maps to a **Yjs Doc** keyed by its relative path (forward-slash normalized). The document holds a single `Y.Text` field named after the file path. File reads and WebSocket clients both operate on this CRDT.

### Control Document

A special `__control__` Yjs Doc stores three namespaced Y-collections:

| Key | Type | Purpose |
|-----|------|---------|
| `tasks` | `Y.Map` | Task records, keyed by task UUID |
| `presence` | `Y.Map` | Agent heartbeat records |
| `lock_log` | `Y.Array` | Append-only lock event log |

### Agent

Any process (AI or human) that connects to a daemon and performs operations. Agents identify themselves by an `agent_id` string passed in request bodies.

---

## Building

**Prerequisites:** Rust 1.83+, Cargo

```bash
git clone <repo>
cd enochian
cargo build
# binaries → target/debug/enochd  target/debug/enoch
```

Release build (recommended for production):

```bash
cargo build --release
# binaries → target/release/enochd  target/release/enoch
```

---

## Configuration

Circle configs are stored at:

```
~/.enochian/circles/<circle-id>/config.toml
```

Example:

```toml
circle_id   = "8e563c41-f0ec-4225-9764-064f1fb04341"
circle_name = "MyCircle"
psk_hex     = "d2d89de6..."        # pre-shared key
keypair_proto_hex = "0802..."      # libp2p Ed25519 keypair (protobuf-encoded hex)
```

This file is created by `enoch init` and read by `enochd serve`.

---

## CLI Reference — `enoch`

All commands accept `--json` for machine-readable output.

```
ENOCHIAN agent CLI — collaborate inside a Circle

Usage: enoch [OPTIONS] <COMMAND>

Options:
  --json    Output raw JSON
  -h, --help

Commands:
  init     Create a new Circle
  enter    Join an existing Circle (P2P dial)
  status   Show Circle overview
  who      Show agent presence
  tasks    List tasks
  claim    Claim a task
  done     Mark a task as done
  bind     Acquire an explicit file lock
  release  Release a file lock
  watch    Stream live Circle events (SSE)
```

### `enoch init`

```bash
enoch init --name <NAME>
```

Creates a new Circle. Generates a fresh Ed25519 keypair and a random 256-bit PSK. Saves config to `~/.enochian/circles/<id>/config.toml`.

**Output:**
```
✦ Circle cast: MyCircle
  circle-id : 8e563c41-f0ec-4225-9764-064f1fb04341
  peer-id   : 12D3KooW...
  secret    : d2d89de6...
```

---

### `enoch enter`

```bash
enoch enter <CIRCLE-ID> --secret <HEX> [--peer <MULTIADDR>] [--rendezvous <MULTIADDR>]
```

Joins an existing Circle. Generates a fresh ephemeral keypair and dials the network.

- On LAN: mDNS discovers peers automatically — no `--peer` needed.
- Direct dial: `--peer /ip4/1.2.3.4/tcp/9091`
- WAN via rendezvous: `--rendezvous /ip4/.../tcp/8888/p2p/12D3KooW...`

---

### `enoch status`

```bash
enoch status
```

```
◆ Circle:  MyCircle
  ID:      8e563c41-...
  SyncDir: ~/.enochian/circles/.../files
  Docs:    3
```

---

### `enoch who`

```bash
enoch who
```

Lists agents who have registered presence in the circle.

---

### `enoch tasks`

```bash
enoch tasks [--status open|claimed|done]
```

```
  [open]    4873c16e  Write integration tests
  [claimed] a2853491  Refactor network layer  (→ agent-beta)
  [done]    f1e2d3c4  Update README
```

---

### `enoch claim`

```bash
enoch claim <TASK-ID>
```

Claims an open task. Sets status to `claimed` and records the agent. The daemon reads `ENOCHIAN_AGENT_ID` env var (defaults to `"anonymous"`) as the claimant.

---

### `enoch done`

```bash
enoch done <TASK-ID>
```

Marks a task as `done`.

---

### `enoch bind`

```bash
enoch bind <PATH>
```

Acquires a file lock on `<PATH>` (relative to the sync dir). Returns HTTP 409 if already held by another agent.

---

### `enoch release`

```bash
enoch release <PATH>
```

Releases the lock on `<PATH>`.

---

### `enoch watch`

```bash
enoch watch
```

Streams live SSE events from the daemon. Blocks until Ctrl+C.

```
◆ Watching circle events (Ctrl+C to stop)...
  [task_created]  {"type":"task_created","task_id":"..."}
  [lock_acquired] {"type":"lock_acquired","path":"src/main.rs","agent_id":"..."}
  [file_updated]  {"type":"file_updated","path":"notes.txt"}
```

---

## Daemon Reference — `enochd`

```
ENOCHIAN daemon — runs the P2P sync node

Usage: enochd serve [OPTIONS] --circle <CIRCLE>

Options:
  --circle <CIRCLE>      Circle ID (UUID)
  --port <PORT>          HTTP port [default: 9090]  (P2P uses port+1)
  --sync-dir <PATH>      Override sync directory
  -h, --help
```

The daemon:
1. Loads the circle config from `~/.enochian/circles/<id>/config.toml`
2. Starts a file watcher on the sync directory
3. Starts the libp2p swarm (P2P on `port+1`)
4. Starts the axum HTTP/WS server (HTTP on `port`)

**Log levels:**

```bash
RUST_LOG=info  enochd serve ...   # normal
RUST_LOG=debug enochd serve ...   # verbose (includes libp2p internals)
RUST_LOG=enochian=debug enochd serve ...  # only enochian crate
```

---

## REST API Reference

Base URL: `http://<host>:<port>/api`

All request bodies are JSON. All responses are JSON.

---

### `GET /api/status`

Circle overview.

**Response:**
```json
{
  "circle_id":   "8e563c41-...",
  "circle_name": "MyCircle",
  "sync_dir":    "/home/user/.enochian/circles/.../files",
  "doc_count":   3
}
```

---

### `GET /api/who`

Agent presence list.

**Response:** `[Presence, ...]`

```json
[
  { "agent_id": "agent-alpha", "status": "active", "last_seen": "2026-05-13T14:00:00Z" }
]
```

---

### `GET /api/tasks?status=<open|claimed|done>`

List tasks. `status` query param is optional.

**Response:** `[Task, ...]` sorted by `created_at` ascending.

```json
[
  {
    "task_id":     "4873c16e-...",
    "title":       "Write integration tests",
    "description": "Cover the lock arbitration logic",
    "status":      "open",
    "created_by":  "agent-alpha",
    "claimed_by":  null,
    "created_at":  "2026-05-13T14:00:00Z",
    "updated_at":  "2026-05-13T14:00:00Z"
  }
]
```

---

### `POST /api/tasks`

Create a task.

**Request:**
```json
{
  "title":       "Write integration tests",
  "description": "Optional description",
  "created_by":  "agent-alpha"
}
```

**Response:** `201 Created`
```json
{ "task_id": "4873c16e-...", "status": "created" }
```

---

### `POST /api/claim`

Claim a task (open → claimed).

**Request:**
```json
{ "task_id": "4873c16e-...", "agent_id": "agent-alpha" }
```

**Response:** `200 OK`
```json
{ "status": "claimed", "task_id": "4873c16e-..." }
```

**Error:** `404` if task not found.

---

### `POST /api/done`

Mark a task done (claimed → done).

**Request:**
```json
{ "task_id": "4873c16e-..." }
```

**Response:** `200 OK`
```json
{ "status": "done", "task_id": "4873c16e-..." }
```

---

### `POST /api/bind`

Acquire a file lock.

**Request:**
```json
{ "path": "src/main.rs", "agent_id": "agent-alpha" }
```

**Response:** `200 OK`
```json
{ "status": "bound", "path": "src/main.rs", "agent_id": "agent-alpha" }
```

**Conflict:** `409 Conflict`
```json
{ "error": "already locked", "held_by": "agent-beta" }
```

Side effect: the physical file is set read-only (`chmod 444` / `SetFileAttributes READONLY`) to prevent accidental overwrites.

---

### `POST /api/release`

Release a file lock.

**Request:**
```json
{ "path": "src/main.rs", "agent_id": "agent-alpha" }
```

**Response:** `200 OK`
```json
{ "status": "released", "path": "src/main.rs" }
```

Side effect: file permissions restored to read-write.

---

### `GET /api/events`

Server-Sent Events stream. Connect and keep open.

```
Content-Type: text/event-stream

data: {"type":"task_created","task_id":"..."}

data: {"type":"lock_acquired","path":"src/main.rs","agent_id":"agent-alpha"}

data: {"type":"file_updated","path":"notes.txt"}
```

---

## WebSocket Sync Protocol

```
ws://<host>:<port>/ws/yjs?path=<relative-file-path>
```

Implements the **y-sync v1 protocol** over binary WebSocket frames.

### Handshake

On connect, the server immediately sends a **SyncStep1** message containing its current state vector:

```
Server → Client: [SyncStep1(state_vector)]
```

The client should respond with a **SyncStep2** containing everything the server is missing, plus its own SyncStep1:

```
Client → Server: [SyncStep2(diff), SyncStep1(client_sv)]
Server → Client: [SyncStep2(server_diff)]
```

After handshake, both sides are up to date.

### Incremental updates

```
Either side → Other: [Update(raw_v1_update_bytes)]
```

Updates are applied immediately to the Y.Doc and broadcast to all other subscribers on the same path.

### Message encoding

All frames are **binary**. Messages are encoded with `EncoderV1` (yrs). The message type byte prefix:

| Prefix | Type |
|--------|------|
| `0x00 0x00` | SyncStep1 |
| `0x00 0x01` | SyncStep2 |
| `0x00 0x02` | Update |

---

## SSE Event Stream

`GET /api/events` returns `text/event-stream`. Each event is a line:

```
data: <json>\n\n
```

### Event types

| `type` | Fields | Trigger |
|--------|--------|---------|
| `task_created` | `task_id` | `POST /api/tasks` |
| `task_claimed` | `task_id`, `agent_id` | `POST /api/claim` |
| `task_done` | `task_id` | `POST /api/done` |
| `lock_acquired` | `path`, `agent_id` | `POST /api/bind` |
| `lock_released` | `path`, `agent_id` | `POST /api/release` |
| `file_updated` | `path` | File watcher detects disk change |

The internal channel has capacity 256. Slow consumers will miss events (broadcast semantics, not queue).

---

## Lock Arbitration

Locks use a **Y.Array append-log** stored in the control document under the key `lock_log`. Every acquire and release is an immutable entry:

```json
{
  "entry_id":  "uuid",
  "agent_id":  "agent-alpha",
  "path":      "src/main.rs",
  "action":    "acquire",
  "ts":        "2026-05-13T14:00:00Z"
}
```

**Arbitration rule** (`src/control/arbitration.rs`):

1. Replay all entries in insertion order.
2. For each `acquire`: if the path has no current holder, record `path → agent_id`.
3. For each `release`: if the holder matches, remove the path.
4. The resulting map is the current lock state.

This is **deterministic** — every node that applies the same Y.Array updates reaches the same lock state, with no coordinator needed. First-writer-wins on concurrent acquires (Yjs Array ordering is stable under merge).

---

## File Sync

```
Disk file  ←──────────────────────────────────→  Y.Text CRDT
           write         flush_to_disk()
           detected ──→  update Y.Text     ──→   WS broadcast
           by watcher    (full replace)
```

### Disk → CRDT (watcher)

`notify` watches the sync directory recursively. On `Modify(Data)` or `Create(File)`:

1. Read the full file contents.
2. Call `get_or_create_doc(rel_path)` to get the `Arc<Doc>`.
3. Open a `TransactionMut`, get the `Y.Text`, replace contents if changed.
4. Dropping the transaction fires `observe_update_v1`, which sends raw bytes to the broadcast channel.
5. All WebSocket clients subscribed to that path receive the `Update` message.

**Self-write suppression:** Before flushing CRDT → disk, `flush_to_disk` sets an `AtomicBool` flag. The watcher checks and ignores the next event for that path, preventing an infinite loop.

### CRDT → Disk (WebSocket updates)

When a WS client sends a `SyncStep2` or `Update`:

1. The update is applied to the `Arc<Doc>` with `transact_mut().apply_update(...)`.
2. `flush_to_disk` writes the Y.Text string back to the physical file.

### Current limitation

Y.Text sync uses **full-text replacement** (not diff-based). This is correct for CRDT semantics but suboptimal for large files. A future version will use `Y.Text` character-level operations natively from editors that speak the y-sync protocol.

---

## P2P Layer

Built on **libp2p 0.56** with the following behaviours:

| Behaviour | Role |
|-----------|------|
| `tcp` + `noise` + `yamux` | Encrypted, multiplexed transport |
| `mdns` | LAN peer discovery (no config needed) |
| `kad` (Kademlia) | WAN DHT routing, peer discovery |
| `identify` | Exchange protocol versions and listen addresses |
| `ping` | Keepalive / latency measurement |
| `rendezvous` (client) | WAN rendezvous point registration |

### Port assignment

```
HTTP / WS  →  --port        (default 9090)
P2P TCP    →  --port + 1    (default 9091)
```

### mDNS (LAN, zero-config)

On startup, the swarm broadcasts mDNS queries on all local interfaces. When a peer is discovered:
1. Their address is added to the Kademlia routing table.
2. If not already connected, the swarm dials them.
3. `Identify` exchange happens automatically on connect.

### WAN (manual dial or rendezvous)

```bash
# Direct dial
enoch enter <circle-id> --secret <hex> --peer /ip4/1.2.3.4/tcp/9091

# Via rendezvous server
enoch enter <circle-id> --secret <hex> \
  --rendezvous /ip4/1.2.3.4/tcp/8888/p2p/12D3KooWRvServer...
```

**Note:** Y-doc sync over P2P (gossiping Yjs updates between daemons) is planned for Phase 4. Currently, agents sync via WebSocket directly to the HTTP server.

---

## Data Model

### Task

```rust
pub struct Task {
    pub task_id:     String,           // UUID v4
    pub title:       String,
    pub description: Option<String>,
    pub status:      TaskStatus,       // Open | Claimed | Done
    pub created_by:  String,
    pub claimed_by:  Option<String>,
    pub created_at:  DateTime<Utc>,
    pub updated_at:  DateTime<Utc>,
}
```

Stored as a JSON string value in the `tasks` Y.Map. Key = `task_id`.

### LockEntry

```rust
pub struct LockEntry {
    pub entry_id: String,          // UUID v4
    pub agent_id: String,
    pub path:     String,          // relative path, forward-slash normalized
    pub action:   LockAction,      // Acquire | Release
    pub ts:       DateTime<Utc>,
}
```

Stored as JSON strings in the `lock_log` Y.Array (append-only).

### Presence

```rust
pub struct Presence {
    pub agent_id:  String,
    pub status:    AgentStatus,    // Active | Idle | Offline
    pub last_seen: DateTime<Utc>,
}
```

Stored as JSON strings in the `presence` Y.Map. Key = `agent_id`.

---

## Directory Layout

```
~/.enochian/
└── circles/
    └── <circle-id>/
        ├── config.toml          # circle config + keypair
        └── files/               # sync directory
            ├── notes.txt
            └── src/
                └── main.rs

D:\workspace\enochian\         # source
├── src/
│   ├── api/
│   │   ├── mod.rs              # axum Router
│   │   ├── events.rs           # GET /api/events  (SSE)
│   │   ├── lock.rs             # /api/bind  /api/release  /api/claim  /api/done
│   │   ├── status.rs           # GET /api/status
│   │   ├── tasks.rs            # GET/POST /api/tasks
│   │   └── who.rs              # GET /api/who
│   ├── bin/
│   │   ├── enoch.rs            # agent CLI entry point
│   │   └── enochd.rs           # daemon entry point
│   ├── commands/
│   │   ├── serve.rs            # `enochd serve` — main daemon loop
│   │   ├── bind.rs             # `enoch bind`
│   │   ├── claim.rs            # `enoch claim`
│   │   ├── done_cmd.rs         # `enoch done`
│   │   ├── release.rs          # `enoch release`
│   │   ├── status.rs           # `enoch status`
│   │   ├── tasks.rs            # `enoch tasks`
│   │   ├── watch.rs            # `enoch watch`
│   │   └── who.rs              # `enoch who`
│   ├── control/
│   │   ├── mod.rs              # Task, LockEntry, Presence, CircleEvent types
│   │   ├── arbitration.rs      # lock log replay
│   │   └── fs_lock.rs          # chmod helper (set_readonly)
│   ├── store/
│   │   └── fs.rs               # flush_to_disk (Y.Text → file)
│   ├── sync_yjs/
│   │   ├── watcher.rs          # notify watcher → Y.Text
│   │   └── ws_handler.rs       # WebSocket y-sync handler
│   ├── cli.rs                  # clap CLI definitions (both binaries)
│   ├── config.rs               # config.toml load/save
│   ├── crypto.rs               # keypair generation + hex encoding
│   ├── lib.rs                  # crate root (pub mod declarations)
│   ├── network/
│   │   └── behaviour.rs        # EnochBehaviour + EnochEvent
│   └── state.rs                # AppState (Arc<Doc> map, broadcast channels)
├── Cargo.toml
├── README.md
├── AGENTS.md
└── DOCS.md                     # this file
```

---

## Environment Variables

| Variable | Default | Used by | Purpose |
|----------|---------|---------|---------|
| `ENOCHIAN_API` | `http://127.0.0.1:9090/api` | `enoch` CLI | Target daemon base URL |
| `ENOCHIAN_AGENT_ID` | `anonymous` | `enoch claim` / `bind` | Agent identifier sent in requests |
| `RUST_LOG` | `warn` | `enochd` | Tracing log filter (e.g. `info`, `debug`, `enochian=debug`) |
