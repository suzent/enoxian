# enoxian

**P2P agent collaboration protocol** — shared files, tasks, and file locks for AI agents and humans working inside a Circle.

> *"Descend not to the API layer to unify agents. Descend to the protocol layer."*

---

## What it is

enoxian lets any agent (AI or human) join a **Circle** — a named workspace with:

- **Real-time file sync** via Yjs CRDT (conflict-free, offline-capable)
- **Task board** for work coordination
- **Advisory file locks** with deterministic arbitration
- **Presence** tracking
- **Live event stream** over SSE

Two binaries:

| Binary | Role |
|--------|------|
| `enoxd` | Long-running daemon — P2P node, HTTP/WS server, file watcher |
| `enox` | Short-lived CLI — agent sends commands to a daemon |

---

## Quick Start

```bash
cargo build

# 1. Create a Circle
./target/debug/enox init --name "my-project"

# 2. Start the daemon
RUST_LOG=info ./target/debug/enoxd

# 3. In another terminal — talk to it
export ENOXIAN_API=http://127.0.0.1:36521
./target/debug/enox status
./target/debug/enox tasks
./target/debug/enox watch
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
| 0 — P2P skeleton | ✅ | `enox init` / `enoxd` / `enox enter`, mDNS, libp2p |
| 1 — Document sync | ✅ | Yjs Y.Text, file watcher, `/ws/yjs` WebSocket |
| 2 — CLI contract | ✅ | `status`, `who`, `tasks`, `claim`, `done`, `bind`, `release`, `watch` |
| 3 — Coordination | ✅ | Lock log, presence, full REST API, SSE events |
| 4 — P2P doc gossip | ⬜ | Sync Control Doc between daemons over libp2p streams |
| 5 — Agent bridge | ⬜ | Cross-user delivery, planner integration |

---

## Documentation

All docs are in the [`docs/`](docs/) folder:

See [docs/index.md](docs/index.md) for the full documentation index.

| Doc | Description |
|-----|-------------|
| [docs/concepts/overview.md](docs/concepts/overview.md) | **Start here** — intuitive walkthrough with diagrams |
| [docs/guide/getting-started.md](docs/guide/getting-started.md) | Build, initialize, first commands |
| [docs/guide/cli.md](docs/guide/cli.md) | Full `enox` command reference |
| [docs/guide/agents.md](docs/guide/agents.md) | How enoxian drives agents (ACP, mentions, memory) |
| [docs/concepts/concepts.md](docs/concepts/concepts.md) | Circle, Agent, Document, Control Doc |
| [docs/concepts/architecture.md](docs/concepts/architecture.md) | System diagram, components, data model |
| [docs/reference/api.md](docs/reference/api.md) | REST API endpoint reference |
| [docs/reference/daemon.md](docs/reference/daemon.md) | `enoxd` reference and configuration |

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
| `reqwest` | 0.12 | HTTP client (enox CLI) |
| `clap` | 4 | CLI argument parsing |
