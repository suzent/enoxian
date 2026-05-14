# ENOCHIAN Roadmap

## What works today

| Feature | Notes |
|---------|-------|
| Circle creation — `enoch init` | Generates keypair + PSK + admin keypair; prints invite link with embedded admin pubkey |
| Invite links — `enochian://v1/<b64>` | No-quote shell-safe URI, expiry enforced, admin pubkey embedded |
| Join — `enoch enter` | Saves config + admin pubkey, workspace created, conflict handling |
| Multi-circle daemon — `enochd` | Loads all enabled circles at startup; one P2P swarm per circle; hot-reload every 10s |
| Circle lifecycle | `enoch disable/enable/leave`; `POST /circles/<id>/stop|start`; per-circle `CancellationToken` |
| Workspace folders | `~/enochian/<name>/` per circle, configurable via `--dir` |
| REST API | Tasks, locks, presence, members, events SSE, lifecycle |
| Yjs CRDT + file watcher | Local file changes sync into CRDT, broadcast to local WS clients |
| WebSocket Yjs sync | Local editor/agent clients can sync documents over WS |
| Name-based circle resolution | `--circle Work` resolves by exact name → prefix → UUID prefix |
| `enoch` CLI | init, enter, invite, circles, status, who, tasks, task-create, claim, done, bind, release, watch, disable, enable, leave, member |
| PSK-enforced transport | `pnet` XSalsa20 applied at TCP layer — cross-circle connections rejected at handshake |
| Live P2P file sync | Bidirectional y-sync over libp2p streams; mDNS auto-discovery; new files sync without reconnect |
| Self-write loop prevention | Shared per-path flags prevent flush_to_disk from triggering re-sync |
| P2P echo prevention | Updates applied from peers use `"p2p"` origin; observer skips forwarding them back |
| Live presence | Daemon writes hostname-based presence entry on start; 30s heartbeat; `enoch who` shows last-seen age |
| Admin & member management | Admin keypair signs member operations; `enoch member list/add/remove/promote`; daemon verifies signatures |

---

## Architecture principles

**No host, no server.** Every peer in a circle is equal:
- The PSK is the membership credential — every peer holds it, any peer can generate invites
- CRDT (Yjs) means there is no authoritative copy — all peers hold the full state
- mDNS handles LAN discovery automatically with no coordination
- Kademlia DHT handles WAN peer discovery without a central server
- The optional `--peer` in an invite is just a bootstrap hint — any online peer's address works, not just the original creator's

The circle exists as long as at least one peer has the config. If any peer is offline, the others continue operating independently and resync when they reconnect.

---

## Milestone plan

### M1 — Workspace folders
**Status: Complete**

Each circle has a named, visible workspace directory (`~/enochian/<circle-name>/` by default).
See [workspace.md](workspace.md) for details.

---

### M2 — Secure network (PSK enforcement)
**Status: Complete**

PSK is now applied to every swarm via `pnet::PnetConfig` + `with_other_transport()`. Nodes with a mismatched PSK fail the handshake before Noise even starts. Applied in both `commands/serve.rs` (daemon swarm) and `commands/enter.rs` (connectivity check). Cross-circle rejection verified on LAN — mDNS discovers all peers but mismatched circles are silently dropped at the PSK layer.

**Tasks:**
- [x] Apply circle PSK to swarm in `commands/serve.rs`
- [x] Apply circle PSK to swarm in `commands/enter.rs` (connectivity check)
- [x] Verify that cross-circle connections are rejected

---

### M3 — Live P2P sync (core protocol)
**Status: Complete**

`libp2p_stream` behaviour is wired into every circle swarm. On `ConnectionEstablished` (dialing side), a `/enochian/sync/1.0.0` stream is opened. A deadlock-free handshake exchanges `SyncStep1`/`SyncStep2` for all currently-open docs. After handshake, local updates are forwarded via the `all_updates` global broadcast channel; incoming updates are applied to the local CRDT and flushed to workspace disk. Verified end-to-end bidirectional file sync on a real LAN (Windows ↔ Mac).

**Implementation:**
- `src/network/sync.rs` — full sync handler; deadlock-free 3-phase handshake; lag recovery via full-state resend
- `src/state.rs` — `all_updates` broadcast + `self_write_flags` shared between watcher and flush_to_disk; observer kept alive with `mem::forget`; `"p2p"` origin filter prevents echo
- `src/store/fs.rs` — `flush_to_disk` uses `state.self_write_flags` (no longer takes a separate flag arg)
- `src/sync_yjs/watcher.rs` — handles Windows rename-sequence creation events (`Name(To)`) in addition to standard data-modify events
- `libp2p-stream = "0.4.0-alpha"` added to dependencies

**Tasks:**
- [x] Add `libp2p_stream` behaviour for `/enochian/sync/1.0.0`
- [x] On `ConnectionEstablished`: open sync stream, run y-sync handshake for all open docs
- [x] Subscribe to `all_updates` broadcast channel, forward to peer stream
- [x] On incoming update: apply to local CRDT, flush to workspace disk
- [x] Handle new docs created after connection established (dynamic doc discovery via all_updates broadcast)
- [x] Fix observer subscription lifetime (`mem::forget` keeps observer registered for doc's lifetime)
- [x] Fix self-write flag isolation (moved into AppState, shared by watcher + flush_to_disk)
- [x] Fix P2P echo loop (`"p2p"` origin on transact_mut_with, filtered in observer)
- [x] Handle Windows file creation via rename sequence (`Name(To)` event kind)

---

### M4 — Circle lifecycle management
**Status: Complete**

All circles load at daemon startup; individual circles can be stopped, started, disabled, and left without restarting the daemon. Hot-reload polls every 10s for newly-enabled circles.

**Implementation:**
- `src/lifecycle.rs` — `spawn_circle()` with `CancellationToken` per circle; all tasks cancel cleanly
- `src/daemon.rs` — `DaemonState` extended with `tokens` map; `insert_circle`, `stop_circle`, `is_active`
- `src/api/lifecycle.rs` — `POST /circles/<id>/stop` and `/start` handlers
- `src/commands/disable.rs` — sets `disabled=true`, best-effort stop
- `src/commands/enable.rs` — sets `disabled=false`, best-effort start
- `src/commands/leave.rs` — confirmation prompt, removes config dir, best-effort stop
- `src/commands/serve.rs` — hot-reload task (10s poll)

**Tasks:**
- [x] Add `disabled: bool` field to `CircleConfig` (default false, `#[serde(default)]`)
- [x] `enochd` skips disabled circles at startup
- [x] `enoch disable` — set flag in config, call `/circles/<id>/stop`
- [x] `enoch enable` — clear flag in config, call `/circles/<id>/start`
- [x] `enoch leave [--yes]` — confirm prompt, delete config dir, call `/circles/<id>/stop`
- [x] `POST /circles/<id>/stop` API endpoint — cancels token, removes from DaemonState
- [x] `POST /circles/<id>/start` API endpoint — reloads config, spawns circle, inserts into DaemonState
- [x] `enoch circles` output shows disabled circles with a `[paused]` marker

---

### M5 — Presence
**Status: Complete**

On startup, each daemon writes a `Presence` entry (`agent_id = hostname-shortpeerid`, status=online, last_seen=now) to the control doc's `presence` Y.Map. A 30-second heartbeat task refreshes `last_seen`. The control doc observer now forwards updates to `all_updates` so presence changes sync live to P2P peers. `enoch who` shows last-seen age and marks agents stale if their heartbeat is > 90s old.

**Implementation:**
- `src/presence.rs` — `local_agent_id()`, `spawn_presence()`, heartbeat loop
- `src/state.rs` — control doc observer wired into `all_updates`
- `src/commands/who.rs` — last-seen age display, stale detection

**Tasks:**
- [x] Write presence entry (agent ID, hostname, timestamp) on daemon start
- [x] Refresh presence heartbeat every 30s via a tokio interval task
- [x] `enoch who` displays live agents with last-seen time

---

### M6 — Admin & member management

See [admin.md](admin.md) for the full design.

**Status: Complete**

Admin keypair is generated at `enoch init` and stored in `admin.key`. The public key is embedded in invite URIs so joining peers save it automatically. Member operations (add, remove, promote) require an admin signature verified by the daemon; the CLI auto-signs from `admin.key` when present.

> ⚠ **Note:** The PSK still governs transport-layer access. The member list is enforced at the API layer but peers do not yet reject connections from removed members at the swarm level — that enforcement is deferred to a future hardening pass.

**Implementation:**
- `src/invite.rs` — extended binary format: optional admin pubkey appended as u16-length-prefixed bytes (backward-compatible)
- `src/commands/init.rs` — generates admin keypair; saves `admin.key`; embeds pubkey in initial invite
- `src/commands/invite.rs` — loads `admin.key` if present, embeds pubkey in invite
- `src/commands/enter.rs` — extracts `admin_pubkey_hex` from invite, saves to `config.toml`
- `src/api/members.rs` — `list_members`, `add_member`, `remove_member`, `promote_member`; all mutating ops verify admin signature via `libp2p::identity::PublicKey::verify`
- `src/commands/member.rs` — `enoch member list/add/remove/promote`; auto-signs with `admin.key`
- `src/control/mod.rs` — `MemberEntry`, `MemberRole`, `MEMBER_LIST_KEY`, `MemberAdded`/`MemberRemoved` events

**Tasks:**
- [x] Admin keypair generated at `enoch init`, stored as `admin.key`
- [x] Invite format carries admin pubkey (backward-compatible extension)
- [x] `enoch enter` saves `admin_pubkey_hex` from invite to `config.toml`
- [x] Member list stored in control doc CRDT (`member_list` Y.Map) — replicated to all peers
- [x] `enoch member list` — show all members and their roles
- [x] `enoch member add <peer-id> [--role admin|member]` — admin-signed add
- [x] `enoch member remove <peer-id>` — admin-signed remove
- [x] `enoch member promote <peer-id>` — promote to admin
- [x] Daemon verifies admin signature on all member write operations

---

### M7 — CLI completeness
**Status: Complete**

**Implementation:**
- `src/commands/tasks.rs` — `create()` function posts to `POST /api/tasks`
- `src/cli.rs` — `TaskCreate { title, description }` subcommand
- `src/commands/serve.rs` — hot-reload loop (10s poll, shared with M4)

**Tasks:**
- [x] `enoch task-create <title> [--description "..."]`
- [x] Hot-reload new circles without restarting `enochd`

---

### M8 — Chat
**Status: Planned**

A persistent, replicated chat channel per circle. Messages are stored in a `chat` Y.Array in the control doc and sync to all peers via the existing CRDT layer — no new protocol needed.

**Data model:**

Each message is a JSON object appended to the array:
```json
{ "id": "<uuid>", "agent_id": "hostname-shortpeer", "text": "...", "ts": 1234567890 }
```

The array is append-only by convention — edits are not supported. Deletes are soft (a `deleted: true` flag).

**API:**

| Endpoint | Description |
|----------|-------------|
| `GET /circles/<id>/api/chat?since=<ts>` | Return messages after a Unix timestamp (default: all) |
| `POST /circles/<id>/api/chat` | Append a message `{ "text": "..." }` |

**CLI:**

| Command | Description |
|---------|-------------|
| `enoch chat` | Print recent messages (last 50) |
| `enoch chat --follow` | Stream new messages as they arrive (SSE) |
| `enoch say "<text>"` | Post a message |

**Tasks:**
- [ ] Add `chat` Y.Array to control doc; define `ChatMessage` struct in `src/control/mod.rs`
- [ ] `GET /api/chat` — read messages, optional `?since=<ts>` filter
- [ ] `POST /api/chat` — append message, emit `CircleEvent::MessagePosted`
- [ ] SSE stream for chat (reuse events infrastructure)
- [ ] `enoch chat` and `enoch chat --follow` commands
- [ ] `enoch say "<text>"` shorthand command

---

### M9 — Frontend
**Status: Planned**

A minimal web UI served by `enochd` itself (no separate build server). Targets local agent use: one browser tab per circle, showing files, tasks, presence, and chat.

**Scope (first cut):**

| Panel | Content |
|-------|---------|
| Sidebar | Circle selector, online members (presence) |
| Files | Directory tree of workspace files; click to open in a Yjs-backed CodeMirror editor (collaborative) |
| Tasks | Task list with claim/done actions |
| Chat | Scrolling message log + send box |

**Serving:**

`enochd` serves the compiled SPA from `static/` via `tower-http::ServeDir` at `/app`. No separate dev server in production — `cargo build` bundles the assets. In development, Vite proxy forwards `/circles/` API calls to the daemon.

**Tech stack:**
- Vite + React + TypeScript
- [y-codemirror.next](https://github.com/yjs/y-codemirror.next) for collaborative editing (connects to existing `/ws/yjs` endpoint)
- `yjs` + `y-protocols` for local CRDT binding
- Tailwind CSS for styling

**Tasks:**
- [ ] `frontend/` Vite + React scaffold; `npm run build` outputs to `static/`
- [ ] `enochd` serves `static/` at `/app` via `tower-http::ServeDir`
- [ ] Circle selector — `GET /circles` to list active circles
- [ ] Presence panel — poll `GET /api/who` every 30s
- [ ] Task panel — list, claim, done
- [ ] Chat panel — load history + SSE stream for live messages; send via `POST /api/chat`
- [ ] File tree — list workspace files via a new `GET /api/files` endpoint
- [ ] Collaborative editor — CodeMirror 6 + y-codemirror bound to `/ws/yjs`
- [ ] Production build step: `npm run build` before `cargo build --release`
