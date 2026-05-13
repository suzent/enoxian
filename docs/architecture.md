# Architecture

## System Diagram

```
┌──────────────────────────────────────────────────────────────┐
│                           enochd                             │
│                                                              │
│  ┌─────────────┐   ┌────────────────────┐   ┌────────────┐  │
│  │  libp2p     │   │   axum HTTP + WS   │   │   notify   │  │
│  │  Swarm      │   │   :9090            │   │   watcher  │  │
│  │             │   │                    │   │            │  │
│  │  TCP/Noise  │   │  GET  /api/status  │   │  disk →    │  │
│  │  Yamux      │   │  GET  /api/who     │   │  Y.Text    │  │
│  │  mDNS       │   │  GET  /api/tasks   │   │            │  │
│  │  Kademlia   │   │  POST /api/tasks   │   │  Y.Text →  │  │
│  │  Identify   │   │  POST /api/claim   │   │  disk      │  │
│  │  Ping       │   │  POST /api/done    │   └────────────┘  │
│  │  Rendezvous │   │  POST /api/bind    │                    │
│  │  :9091      │   │  POST /api/release │   ┌────────────┐  │
│  └─────────────┘   │  GET  /api/events  │   │  AppState  │  │
│         │          │  WS   /ws/yjs      │   │            │  │
│         │          └────────────────────┘   │ docs       │  │
│         │                   │               │ Arc<Doc>×N │  │
│         │                   │               │            │  │
│  P2P gossip          HTTP / WS              │ control    │  │
│  (future)            reqwest                │ Arc<Doc>   │  │
│         │                   │               │            │  │
└─────────────────────────────────────────────────────────────┘
          │                   │
          ▼                   ▼
  ┌──────────────┐   ┌──────────────────┐
  │  enochd peer │   │    enoch CLI     │
  │  (other node)│   │  or AI agent     │
  └──────────────┘   └──────────────────┘
```

---

## Component Map

| Component | Source | Responsibility |
|-----------|--------|----------------|
| `AppState` | `src/state.rs` | Central shared state; `Clone` is cheap (all `Arc`) |
| REST/WS router | `src/api/mod.rs` | Builds the axum `Router`, wires state |
| Status handler | `src/api/status.rs` | `GET /api/status` |
| Who handler | `src/api/who.rs` | `GET /api/who` — reads presence Y.Map |
| Tasks handler | `src/api/tasks.rs` | `GET/POST /api/tasks` — reads/writes tasks Y.Map |
| Lock handler | `src/api/lock.rs` | bind, release, claim, done |
| SSE handler | `src/api/events.rs` | `GET /api/events` — bridges broadcast channel to HTTP |
| WS sync | `src/sync_yjs/ws_handler.rs` | y-sync protocol over WebSocket |
| File watcher | `src/sync_yjs/watcher.rs` | `notify` events → Y.Text mutations |
| Disk flush | `src/store/fs.rs` | Y.Text string → file write |
| Lock arbitration | `src/control/arbitration.rs` | Replay `lock_log` array → current holder map |
| Control types | `src/control/mod.rs` | Task, LockEntry, Presence, CircleEvent structs |
| FS lock | `src/control/fs_lock.rs` | `set_readonly()` — chmod wrapper |
| Serve command | `src/commands/serve.rs` | Main daemon loop; swarm + axum via `tokio::select!` |
| CLI commands | `src/commands/*.rs` | `reqwest` calls to the REST API |
| CLI definitions | `src/cli.rs` | `clap` arg structs for both binaries |

---

## AppState

`AppState` is the single struct threaded through all axum handlers via `State<AppState>`. It is `Clone + Send + Sync + 'static` — required by axum's `Handler` trait.

```rust
#[derive(Clone)]
pub struct AppState {
    pub circle_id:   String,
    pub circle_name: String,
    pub sync_dir:    PathBuf,

    /// File docs. Key = relative path with forward slashes.
    pub docs:        Arc<DashMap<String, Arc<Doc>>>,

    /// __control__ coordination document (tasks, presence, lock_log)
    pub control:     Arc<Doc>,

    /// Per-doc update broadcast (raw v1 bytes → WS clients)
    pub doc_updates: Arc<DashMap<String, broadcast::Sender<Vec<u8>>>>,

    /// SSE event stream
    pub events:      broadcast::Sender<CircleEvent>,
}
```

**Key design decisions:**

- `Arc<Doc>` (not `Arc<RwLock<Awareness>>`): `yrs::Awareness` stores `dyn Fn(...)` callbacks without a `Send` bound, making it non-`Send`. `yrs::Doc` is internally `Arc`-based and `Send + Sync`.
- `DashMap`: lock-free concurrent HashMap, so handlers don't need to hold a mutex while doing async work.
- Broadcast channels: `observe_update_v1` fires synchronously on `TransactionMut` drop, sends raw update bytes to a `tokio::sync::broadcast::Sender`. WS handlers subscribe and forward.

---

## Data Model

### Task

```rust
pub struct Task {
    pub task_id:     String,
    pub title:       String,
    pub description: Option<String>,
    pub status:      TaskStatus,       // Open | Claimed | Done
    pub created_by:  String,
    pub claimed_by:  Option<String>,
    pub created_at:  DateTime<Utc>,
    pub updated_at:  DateTime<Utc>,
}
```

Stored as a JSON string in the `tasks` Y.Map. Key = `task_id`.

### LockEntry

```rust
pub struct LockEntry {
    pub entry_id: String,
    pub agent_id: String,
    pub path:     String,            // relative, forward-slash normalized
    pub action:   LockAction,        // Acquire | Release
    pub ts:       DateTime<Utc>,
}
```

Stored as JSON strings in the `lock_log` Y.Array (append-only).

### Presence

```rust
pub struct Presence {
    pub agent_id:  String,
    pub status:    AgentStatus,      // Active | Idle | Offline
    pub last_seen: DateTime<Utc>,
}
```

Stored as JSON strings in the `presence` Y.Map. Key = `agent_id`.

### CircleEvent

```rust
pub enum CircleEvent {
    TaskCreated  { task_id: String },
    TaskClaimed  { task_id: String, agent_id: String },
    TaskDone     { task_id: String },
    LockAcquired { path: String, agent_id: String },
    LockReleased { path: String, agent_id: String },
    FileUpdated  { path: String },
}
```

Serialized to JSON (`{"type":"task_created", ...}`) for the SSE stream.

---

## Directory Layout

### Runtime (data)

```
~/.enochian/
└── circles/
    └── <circle-id>/
        ├── config.toml          # Circle config + keypair
        └── files/               # Sync directory (watched)
            ├── notes.txt
            └── src/
                └── main.rs
```

### Source

```
src/
├── bin/
│   ├── enoch.rs                 # Agent CLI entry point
│   └── enochd.rs                # Daemon entry point
├── api/
│   ├── mod.rs                   # axum Router
│   ├── events.rs                # GET /api/events
│   ├── lock.rs                  # /api/bind  /api/release  /api/claim  /api/done
│   ├── status.rs                # GET /api/status
│   ├── tasks.rs                 # GET/POST /api/tasks
│   └── who.rs                   # GET /api/who
├── commands/
│   ├── serve.rs                 # enochd serve — main loop
│   ├── bind.rs                  # enoch bind
│   ├── claim.rs                 # enoch claim
│   ├── done_cmd.rs              # enoch done
│   ├── release.rs               # enoch release
│   ├── status.rs                # enoch status
│   ├── tasks.rs                 # enoch tasks
│   ├── watch.rs                 # enoch watch
│   └── who.rs                   # enoch who
├── control/
│   ├── mod.rs                   # Task, LockEntry, Presence, CircleEvent
│   ├── arbitration.rs           # Lock log replay
│   └── fs_lock.rs               # set_readonly()
├── store/
│   └── fs.rs                    # flush_to_disk (Y.Text → file)
├── sync_yjs/
│   ├── watcher.rs               # notify watcher → Y.Text
│   └── ws_handler.rs            # WebSocket y-sync handler
├── cli.rs                       # clap CLI definitions (both binaries)
├── config.rs                    # config.toml load/save
├── crypto.rs                    # Keypair generation + hex encoding
├── lib.rs                       # Crate root
├── network/
│   └── behaviour.rs             # EnochBehaviour + EnochEvent (libp2p)
└── state.rs                     # AppState
```

---

## Dependency Stack

| Crate | Version | Role |
|-------|---------|------|
| `tokio` | 1 | Async runtime |
| `axum` | 0.8 | HTTP + WebSocket server |
| `libp2p` | 0.56 | P2P transport and protocols |
| `yrs` | 0.26 | Yjs CRDT (Y.Doc, Y.Text, Y.Map, Y.Array) |
| `notify` | 8 | Cross-platform file watcher |
| `dashmap` | 6 | Lock-free concurrent HashMap |
| `reqwest` | 0.12 | HTTP client (enoch CLI) |
| `serde` / `serde_json` | 1 | Serialization |
| `clap` | 4 | CLI argument parsing |
| `tokio-stream` | 0.1 | Broadcast → SSE stream bridging |
| `tower-http` | 0.6 | CORS, tracing middleware |
| `tracing` / `tracing-subscriber` | 0.1 / 0.3 | Structured logging |
| `chrono` | 0.4 | Timestamps |
| `uuid` | 1 | UUID v4 generation |
| `anyhow` | 1 | Error handling |
