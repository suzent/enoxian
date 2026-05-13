# enochd — ENOCHIAN Daemon

**ENOCHIAN** is a P2P agent collaboration protocol. Any agent that can connect via WebSocket can join a Circle — no VPN, no admin rights, no code changes required.

> *"Descend not to the API layer to unify agents. Descend to the protocol layer."*

---

## Architecture

```
┌─────────────────────────────────┐
│        Application Layer        │  ← Suzent UI / Claude Code / any agent
├─────────────────────────────────┤
│      Coordination Layer         │  ← __control__ (locks / tasks / presence)
├─────────────────────────────────┤
│      Document Sync Layer        │  ← Yjs CRDT (real-time file sync)  [Phase 1]
├─────────────────────────────────┤
│      Transport Layer (libp2p)   │  ← Noise TLS + TCP/WebSocket + Yamux
├─────────────────────────────────┤
│      Discovery Layer            │  ← mDNS (LAN) + Rendezvous (WAN)
└─────────────────────────────────┘
```

`enochd` is the daemon that runs this stack. A single binary, single port (`:9090`).

---

## Quick Start

### 1. Create a Circle

```bash
enochd init --name "my-project"
```

Output:
```
✦ Circle cast: my-project
  circle-id : c7f3a...
  peer-id   : 12D3Koo...
  secret    : a3f0ed...

Config saved to ~/.enochian/circles/c7f3a.../config.toml
```

### 2. Start the Daemon (Keeper node)

```bash
enochd serve --circle <circle-id>
```

### 3. Join from Another Machine

```bash
# LAN — mDNS auto-discovery, no extra config
enochd enter <circle-id> --secret <psk>

# Direct dial — when mDNS is blocked (Windows Firewall, different subnets)
enochd enter <circle-id> --secret <psk> --peer /ip4/192.168.1.10/tcp/9090

# WAN — via rendezvous server
enochd enter <circle-id> --secret <psk> --rendezvous /dns4/rendezvous.suzent.app/tcp/443/wss
```

---

## Build

```bash
cargo build --release
# Binary: target/release/enochd
```

**Requirements:** Rust 1.83+, no external C dependencies (pure Rust crypto via `ring`).

---

## Tech Stack

| Component | Crate | Role |
|-----------|-------|------|
| Async runtime | `tokio` 1.x | Event loop |
| P2P networking | `libp2p` 0.56 | Transport, discovery, NAT traversal |
| CRDT sync | `yrs` 0.26 *(Phase 1)* | Real-time document sync |
| HTTP + WebSocket | `axum` 0.8 *(Phase 1)* | REST API + WS endpoints |
| File watching | `notify` 8.x *(Phase 1)* | inotify / FSEvents / ReadDirectoryChangesW |
| CLI | `clap` 4.x | Subcommand parsing |

---

## Ports & Paths

| Path | Protocol | Purpose |
|------|----------|---------|
| `:9090/ws/peer` | WebSocket | libp2p P2P transport |
| `:9090/ws/yjs` | WebSocket | Yjs CRDT sync *(Phase 1)* |
| `:9090/api/` | HTTP REST | Status, locks, tasks *(Phase 1)* |

---

## Config

Circle configs live in `~/.enochian/circles/<circle-id>/config.toml`:

```toml
circle_id = "c7f3a..."
circle_name = "my-project"
psk_hex = "a3f0ed..."        # 32-byte pre-shared key (hex)
keypair_proto_hex = "..."    # Ed25519 keypair (libp2p protobuf encoding)
```

---

## Phases

| Phase | Status | Scope |
|-------|--------|-------|
| 0 — P2P Skeleton | ✅ Done | `init` / `serve` / `enter`, mDNS, libp2p |
| 1 — Doc Sync | 🔜 Next | Yjs CRDT, file watching, `:9090/ws/yjs` |
| 2 — CLI Contract | ⬜ | `status`, `who`, `tasks`, `claim`, `done`, `bind` |
| 3 — Coordination | ⬜ | Lock log, presence, REST API |
| 4 — Suzent Native | ⬜ | y-py direct, streaming, planner |
| 5 — Agent Bridge | ⬜ | Cross-user delivery, bridge contract |

See [`docs/SPEC.md`](docs/SPEC.md) for the full protocol specification.

---

## For AI Agents

See [`AGENTS.md`](AGENTS.md) for the collaboration contract: when to claim tasks, how to acquire file locks, and how to report completion.
