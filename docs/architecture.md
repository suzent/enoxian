# Architecture

## System Diagram

```
┌─────────────────────────────────────────────────────────────┐
│                     enoxd (circle mode)                    │
│                                                             │
│  ┌─────────────┐   ┌────────────────────┐   ┌────────────┐  │
│  │  libp2p     │   │   axum HTTP + WS   │   │   notify   │  │
│  │  Swarm      │   │   :36521           │   │   watcher  │  │
│  │             │   │                    │   │            │  │
│  │  PSK/Noise  │   │  GET  /api/status  │   │  disk →    │  │
│  │  Yamux      │   │  GET  /api/who     │   │  Y.Text    │  │
│  │  mDNS       │   │  GET  /api/tasks   │   │            │  │
│  │  Kademlia   │   │  POST /api/tasks   │   │  Y.Text →  │  │
│  │  Identify   │   │  POST /api/claim   │   │  disk      │  │
│  │  Ping       │   │  POST /api/done    │   └────────────┘  │
│  │  Rendezvous │   │  POST /api/bind    │                   │
│  │  Relay/DCUtR│   │  POST /api/release │   ┌────────────┐  │
│  │  QUIC (no   │   │  GET  /api/events  │   │  AppState  │  │
│  │   PSK)      │   │  WS   /ws/yjs      │   │            │  │
│  │  :random    │   └────────────────────┘   │ docs       │  │
│  └─────────────┘            │               │ Arc<Doc>×N │  │
│         │                   │               │            │  │
│  PSK TCP: circle peers      HTTP / WS       │ control    │  │
│  QUIC: bootstrap server     reqwest         │ Arc<Doc>   │  │
│  /enoxian/sync                             │            │  │
│  y-sync protocol                            │ peer_id    │  │
│  (live, M3)                                 │ ext_addrs  │  │
│         │                   │               │            │  │
└─────────────────────────────────────────────────────────────┘
          │                   │
          ▼                   ▼
  ┌──────────────┐   ┌──────────────────┐   ┌──────────────────┐
  │  enoxd peer │   │    enox CLI     │   │ enoxd           │
  │  (other node)│   │  or AI agent     │   │ --bootstrap      │
  └──────────────┘   └──────────────────┘   │ (QUIC only,      │
                                            │ no PSK,          │
                                            │ rendezvous +     │
                                            │ relay server)    │
                                            └──────────────────┘
```

---

## Component Map

| Component | Source | Responsibility |
|-----------|--------|----------------|
| `AppState` | `src/state.rs` | Central shared state; `Clone` is cheap (all `Arc`). Includes `peer_id` and `p2p_external_addrs`. |
| REST/WS router | `src/api/mod.rs` | Builds the axum `Router`, wires state |
| Status handler | `src/api/status.rs` | `GET /api/status` — includes `p2p` section: peer_id, external_addrs, listen_addrs, relay_addrs, rendezvous_addrs |
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
| P2P sync | `src/network/sync.rs` | `/enoxian/sync/1.0.0` stream handler; 3-phase handshake + continuous update exchange |
| P2P behaviour | `src/network/behaviour.rs` | `EnochBehaviour` combining all libp2p behaviours (mDNS, Kad, Identify, Ping, Rendezvous client, RelayClient, Relay, DCUtR, Stream) |
| Bootstrap behaviour | `src/network/bootstrap_behaviour.rs` | `BootstrapBehaviour` for `enoxd --bootstrap`: Rendezvous server + Relay + Identify + Ping + Kad |
| Bootstrap server | `src/bootstrap.rs` | `enoxd --bootstrap` — QUIC-only rendezvous + relay node; no PSK; no circles |
| Serve command | `src/commands/serve.rs` | Main daemon loop; one swarm per circle + axum HTTP server |
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
    pub workspace:   PathBuf,

    /// This node's libp2p peer ID (string form).
    pub peer_id:     String,

    /// Externally-confirmed TCP multiaddrs, populated by SwarmEvent::ExternalAddrConfirmed.
    /// Used by `enox invite` to auto-embed a connectable peer address.
    pub p2p_external_addrs: Arc<RwLock<Vec<String>>>,

    /// Local listen multiaddrs (non-loopback, non-unspecified, non-circuit).
    /// On a VPS these include the real public IP immediately at startup, before any peer
    /// connects to confirm via Identify. Used as fallback in `enox invite`.
    pub p2p_listen_addrs: Arc<RwLock<Vec<String>>>,

    /// File docs. Key = relative path with forward slashes.
    pub docs:        Arc<DashMap<String, Arc<Doc>>>,

    /// __control__ coordination document (tasks, presence, lock_log)
    pub control:     Arc<Doc>,

    /// Per-doc update broadcast (raw v1 bytes → WS clients)
    pub doc_updates: Arc<DashMap<String, broadcast::Sender<Vec<u8>>>>,

    /// Global broadcast: (rel_path, raw_v1_update) → P2P sync tasks
    pub all_updates: broadcast::Sender<(String, Vec<u8>)>,

    /// SSE event stream
    pub events:      broadcast::Sender<CircleEvent>,

    /// Per-path flag shared between flush_to_disk and the file watcher.
    /// Set to true before writing, cleared by the watcher to skip its own writes.
    pub self_write_flags: Arc<DashMap<String, Arc<AtomicBool>>>,
}
```

**Key design decisions:**

- `Arc<Doc>` (not `Arc<RwLock<Awareness>>`): `yrs::Awareness` stores `dyn Fn(...)` callbacks without a `Send` bound, making it non-`Send`. `yrs::Doc` is internally `Arc`-based and `Send + Sync`.
- `DashMap`: lock-free concurrent HashMap, so handlers don't need to hold a mutex while doing async work.
- `doc_updates` broadcast: `observe_update_v1` fires synchronously on `TransactionMut` drop, sends raw update bytes. WS handlers subscribe and forward.
- `all_updates` broadcast: same observer also sends `(path, bytes)` to this channel, but only for locally-originated updates (origin ≠ `"p2p"`). P2P sync tasks subscribe here to forward to peers without echoing received updates back.
- `self_write_flags` in `AppState`: both the file watcher and `flush_to_disk` reference the same flag map so the suppress-self-write handshake works across tasks.
- `p2p_external_addrs`: a plain `std::sync::RwLock` (not async) because writes are rare events (address confirmation) and reads are short (status endpoint only).
- Observer lifetime: the `Subscription` returned by `observe_update_v1` is kept alive via `std::mem::forget` — dropping it would silently unregister the observer.

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
~/.enoxian/
└── circles/
    └── <circle-id>/
        ├── config.toml          # Circle config, node keypair, PSK, workspace path
        └── admin.key            # Admin Ed25519 keypair hex (creator only; unenforced until M6)

~/enoxian/                      # Default workspace root (configurable via --dir or workspace_dir)
└── <circle-name>/               # One directory per circle
    ├── notes.txt
    └── src/
        └── main.rs
```

The workspace path is stored in `config.toml` as `workspace_dir` and can be any directory on the filesystem. The config dir (`~/.enoxian/circles/<id>/`) holds credentials only — workspace files live separately so they're easy to find and edit directly.

### Source

```
src/
├── bin/
│   ├── enox.rs                 # Agent CLI entry point
│   └── enoxd.rs                # Daemon entry point
├── api/
│   ├── mod.rs                   # axum Router
│   ├── events.rs                # GET /api/events
│   ├── lock.rs                  # /api/bind  /api/release  /api/claim  /api/done
│   ├── status.rs                # GET /api/status
│   ├── tasks.rs                 # GET/POST /api/tasks
│   └── who.rs                   # GET /api/who
├── commands/
│   ├── serve.rs                 # enoxd main loop
│   ├── bind.rs                  # enox bind
│   ├── claim.rs                 # enox claim
│   ├── done_cmd.rs              # enox done
│   ├── release.rs               # enox release
│   ├── status.rs                # enox status
│   ├── tasks.rs                 # enox tasks
│   ├── watch.rs                 # enox watch
│   └── who.rs                   # enox who
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
├── bootstrap.rs                 # enoxd --bootstrap server (QUIC rendezvous + relay)
├── network/
│   ├── behaviour.rs             # EnochBehaviour + EnochEvent (libp2p)
│   ├── bootstrap_behaviour.rs   # BootstrapBehaviour + BootstrapEvent (--bootstrap mode)
│   └── sync.rs                  # /enoxian/sync/1.0.0 — y-sync over libp2p Stream
└── state.rs                     # AppState
```

---

## Transport Stack

Each circle swarm uses three transport legs combined via `or_transport`:

| Transport | Multiaddr pattern | PSK | Purpose |
|-----------|-------------------|-----|---------|
| TCP + PSK (XSalsa20) + Noise + Yamux | `/ip4/.../tcp/...` | ✅ required | Direct connections between circle members on LAN or WAN |
| Circuit relay (Noise + Yamux, no PSK) | `/ip4/.../tcp/.../p2p/.../p2p-circuit` | ❌ | Inbound relay circuits for peers behind NAT |
| QUIC (no PSK) | `/ip4/.../udp/.../quic-v1` | ❌ | Connections to bootstrap/rendezvous servers |

The PSK is transport-level: it runs before Noise. Bootstrap servers do not know any circle's PSK, so they are unreachable over the PSK-TCP leg. Circle members reach them exclusively over QUIC.

The bootstrap server (`enoxd --bootstrap`) runs **QUIC only** — it never participates in a circle and holds no PSK.

The axum HTTP/WebSocket server is a privileged local control plane for the CLI
and browser UI. It is not the WAN relay path. `/ws/yjs` syncs local browser
clients with the local daemon; cross-machine file sync uses the libp2p stream
protocol (`/enoxian/sync/1.0.0`).

---

## Dependency Stack

| Crate | Version | Role |
|-------|---------|------|
| `tokio` | 1 | Async runtime |
| `axum` | 0.8 | HTTP + WebSocket server |
| `libp2p` | 0.56 | P2P transport and protocols (tcp, quic, pnet, noise, yamux, mdns, kad, identify, ping, rendezvous, relay, dcutr) |
| `libp2p-stream` | 0.4.0-alpha | Custom stream protocol (`/enoxian/sync/1.0.0`) |
| `tokio-util` | 0.7 | `FuturesAsyncReadCompatExt` — bridges libp2p Stream to tokio AsyncRead/Write |
| `yrs` | 0.26 | Yjs CRDT (Y.Doc, Y.Text, Y.Map, Y.Array) |
| `notify` | 8 | Cross-platform file watcher |
| `dashmap` | 6 | Lock-free concurrent HashMap |
| `reqwest` | 0.12 | HTTP client (enox CLI) |
| `serde` / `serde_json` | 1 | Serialization |
| `clap` | 4 | CLI argument parsing |
| `tokio-stream` | 0.1 | Broadcast → SSE stream bridging |
| `tower-http` | 0.6 | CORS, tracing middleware |
| `tracing` / `tracing-subscriber` | 0.1 / 0.3 | Structured logging |
| `chrono` | 0.4 | Timestamps |
| `uuid` | 1 | UUID v4 generation |
| `anyhow` | 1 | Error handling |
