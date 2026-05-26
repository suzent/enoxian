# ENOCHIAN

**P2P agent collaboration protocol** — shared files, tasks, and file locks for AI agents and humans working inside a Circle.

> *"Descend not to the API layer to unify agents. Descend to the protocol layer."*

---

## What it is

ENOCHIAN lets any agent (AI or human) join a **Circle** — a named workspace with:

- **Real-time file sync** via Yjs CRDT (conflict-free, offline-capable)
- **Task board** for work coordination
- **Advisory file locks** with deterministic arbitration
- **Presence** tracking
- **Live event stream** over SSE

Two binaries:

| Binary | Role |
|--------|------|
| `enochd` | Long-running daemon — P2P node, HTTP/WS server, file watcher |
| `enoch` | Short-lived CLI — agent sends commands to a daemon |

---

## Quick Start

```bash
cargo build

# 1. Create a Circle
./target/debug/enoch init --name "my-project"

# 2. Start the daemon
RUST_LOG=info ./target/debug/enochd serve --circle <circle-id>

# 3. In another terminal — talk to it
export ENOCHIAN_API=http://127.0.0.1:9090/api
./target/debug/enoch status
./target/debug/enoch tasks
./target/debug/enoch watch
```

---

## Protocol Stack

```
┌─────────────────────────────────┐
│        Application Layer        │  agents, AI models, scripts
├─────────────────────────────────┤
│      Coordination Layer         │  tasks / locks / presence (Control Doc)
├─────────────────────────────────┤
│      Document Sync Layer        │  Yjs CRDT — real-time file sync
├─────────────────────────────────┤
│      Transport Layer            │  TCP + Noise + Yamux (libp2p)
├─────────────────────────────────┤
│      Discovery Layer            │  mDNS (LAN) + Kademlia (WAN)
└─────────────────────────────────┘
```

---

## Implementation Status

| Phase | Status | Scope |
|-------|--------|-------|
| 0 — P2P skeleton | ✅ | `enoch init` / `enochd serve` / `enoch enter`, mDNS, libp2p |
| 1 — Document sync | ✅ | Yjs Y.Text, file watcher, `/ws/yjs` WebSocket |
| 2 — CLI contract | ✅ | `status`, `who`, `tasks`, `claim`, `done`, `bind`, `release`, `watch` |
| 3 — Coordination | ✅ | Lock log, presence, full REST API, SSE events |
| 4 — P2P doc gossip | ⬜ | Sync Control Doc between daemons over libp2p streams |
| 5 — Agent bridge | ⬜ | Cross-user delivery, planner integration |

---

## Documentation

All docs are in the [`docs/`](docs/) folder:

| Doc | Description |
|-----|-------------|
| [docs/overview.md](docs/overview.md) | **Start here** — intuitive walkthrough with diagrams |
| [docs/getting-started.md](docs/getting-started.md) | Build, initialize, first commands |
| [docs/concepts.md](docs/concepts.md) | Circle, Agent, Document, Control Doc |
| [docs/cli.md](docs/cli.md) | Full `enoch` command reference |
| [docs/daemon.md](docs/daemon.md) | `enochd` reference and configuration |
| [docs/api.md](docs/api.md) | REST API endpoint reference |
| [docs/protocol.md](docs/protocol.md) | WebSocket y-sync and SSE event stream |
| [docs/architecture.md](docs/architecture.md) | System diagram, components, data model |
| [docs/internals.md](docs/internals.md) | Lock arbitration, file sync, P2P layer |

For AI agents: see [AGENTS.md](AGENTS.md).

---

## Tech Stack

| Crate | Version | Role |
|-------|---------|------|
| `tokio` | 1 | Async runtime |
| `libp2p` | 0.56 | P2P transport, mDNS, Kademlia |
| `yrs` | 0.26 | Yjs CRDT (Y.Text, Y.Map, Y.Array) |
| `axum` | 0.8 | HTTP + WebSocket server |
| `notify` | 8 | Cross-platform file watcher |
| `reqwest` | 0.12 | HTTP client (enoch CLI) |
| `clap` | 4 | CLI argument parsing |
