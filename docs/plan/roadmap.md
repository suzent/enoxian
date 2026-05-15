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
| File sync hardening | Startup preload; post-handshake full-state push; macOS atomic-save fix (`Name(Both)`); temp file filter; CRDT state persistence across restarts |
| Self-write loop prevention | Shared per-path flags prevent flush_to_disk from triggering re-sync |
| P2P echo prevention | Updates applied from peers use `"p2p"` origin; observer skips forwarding them back |
| Live presence | Daemon writes hostname-based presence entry on start; 30s heartbeat; `enoch who` shows last-seen age |
| Admin & member management | Admin keypair signs member operations; `enoch member list/add/remove/promote`; daemon verifies signatures |
| Chat | `enoch say` posts messages; `enoch chat [--follow]` reads/streams; `@mention` emits `AgentMentioned` event as agent wake signal |

---

## Architecture principles

See [security.md](../security.md) for the full threat model, including PSK semantics, peer identity guarantees, and current limitations of member removal.

**No single host, no mandatory server.** Every peer in a circle is equal:
- The PSK is the network-layer membership credential — every peer holds it
- The admin keypair is the authority for membership operations — only the admin can add/remove members
- CRDT (Yjs) means there is no authoritative copy for real-time edits — all peers hold the full state
- mDNS handles LAN discovery automatically with no coordination
- Kademlia DHT + rendezvous handles WAN peer discovery
- The optional `--peer` in an invite is just a bootstrap hint — any online peer's address works

**Anchor nodes** are regular `enochd` peers that happen to run 24/7 on a VPS. They act as relay, rendezvous point, and always-on presence for their circles. No special code — just a peer that's always reachable. Teams that need WAN or strong liveness guarantees deploy one; LAN-only teams don't need it.

**Conflict model:** CRDT handles concurrent real-time edits perfectly. For offline edits (both peers disconnected simultaneously), conflict detection uses the persisted CRDT state as the common ancestor. If only one side diverged, the merge is clean. If both sides diverged, the loser's version is preserved as a conflict copy (`file.conflict.<peer>`) and the CRDT merge becomes the working file.

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
- [x] Pre-load all workspace files into CRDT on startup (so handshake includes all local docs)
- [x] Post-handshake full-state push (both sides send full CRDT state after handshake — fixes asymmetric doc sets)
- [x] Handle macOS atomic-save rename (`Name(Both)` event — fixes Mac→Windows sync)
- [x] Temp file filter (`is_ignored` — Sublime Text `.sb-*`, Vim `.swp`, hidden files, etc.)
- [x] CRDT state persistence (`store/crdt.rs` — saves binary state to `.enoch_crdt/` after every update; restores on restart to preserve operation IDs and prevent content duplication)

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

> ⚠ **Security note:** The member list is a directory, not an access gate. Removing a peer from the list stops them from auto-registering on restart but does not revoke their PSK — they can still connect and sync, or rejoin with a fresh keypair. True access revocation requires PSK rotation (planned M11). See [security.md](../security.md) for the full threat model.

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
- [x] Auto-registration — each peer writes its own member entry to the CRDT on daemon start (role=Admin if `admin.key` present, Member otherwise); skips if entry already exists so explicit removals persist across restarts
- [x] Peer ID prefix/suffix resolution in `member remove` and `member promote` (short suffix from `enoch member list` accepted directly)

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

### M8 — File sync hardening
**Status: In Progress**

Robust conflict detection and circle liveness tracking. The CRDT handles real-time concurrent edits perfectly; this milestone handles the offline-edit case where both peers diverge from a common ancestor while disconnected.

**Conflict resolution model:**

```
Both peers online          → CRDT merge (perfect, no conflict)
One peer was offline       → offline peer catches up, no conflict
Both peers were offline    → detect via session tracking:
  only one side edited     → clean merge
  both sides edited        → CRDT merge attempt + conflict copy for the losing version
```

**Session & liveness tracking:**

Each peer tracks a `session_id` (incremented on every daemon start) and `last_connected_at`. On reconnect, peers exchange these to determine who was offline and whether both sides diverged.

**Implementation (done):**
- `src/store/crdt.rs` — CRDT state persistence and restore
- `src/sync_yjs/watcher.rs` — startup preload, macOS `Name(Both)` fix, temp file filter
- `src/network/sync.rs` — post-handshake full-state push

**Tasks:**
- [x] CRDT state persistence across restarts (`.enoch_crdt/`)
- [x] Startup workspace preload
- [x] Post-handshake full-state push
- [x] macOS atomic-save fix (`Name(Both)` rename event)
- [x] Temp/hidden file filter
- [ ] Session ID — increment on each daemon start, store in circle state
- [ ] `last_connected_at` — updated on every swarm `ConnectionEstablished` event
- [ ] Exchange session metadata on reconnect (via control doc or handshake extension)
- [ ] Conflict detection — compare both sides' CRDT state against common ancestor (persisted state)
- [ ] Conflict copy — when both sides diverged, write `<file>.conflict.<agent_id>` and keep CRDT merge as working file
- [ ] `enoch status` shows unresolved conflict files in the workspace

**Planned: Binary file dual-track sync**

Text files sync via Yjs (sequence CRDT). Binary and large files need a different path: content-addressed blob sync layered on top of the same P2P transport.

```
Text/code files   → Yjs CRDT (operation-level merge, real-time collaboration)
Binary/large files → Blob sync (hash ref in Yjs CRDT → fetch content by hash via libp2p)
```

Design:
- On write: hash the binary file (BLAKE3) → store as `.enoch_blobs/<hash>` → write the hash string into the Yjs doc for that path
- On sync: peer receives a hash ref → checks if it has the blob locally → if not, fetches via a new `/enochian/blob/1.0.0` libp2p stream protocol
- The Yjs CRDT remains the source of truth for "which version is current" (the hash); the blob store is a content-addressed cache
- Large text files (> configurable threshold, default 1 MB) can also go through the blob path to avoid bloating CRDT state

Tasks (to be scheduled as part of M8 or a dedicated M8.5):
- [ ] BLAKE3 blob store (`store/blobs.rs`) — `put(bytes) → hash`, `get(hash) → bytes`, stored in `.enoch_blobs/`
- [ ] Binary/large file detection in watcher — route to blob store instead of Yjs text encoding
- [ ] Hash ref format in Yjs — store `blob:<hash>` as the doc content for binary paths
- [ ] `/enochian/blob/1.0.0` stream protocol — request/response for blob fetch by hash
- [ ] On P2P sync: detect `blob:` refs in received docs → request missing blobs from peer

---

### M9 — Chat
**Status: Complete**

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

**Implementation:**
- `src/control/mod.rs` — `ChatMessage` struct (`id`, `agent_id`, `text`, `mentions`, `ts`); `CHAT_KEY` constant; `MessagePosted` and `AgentMentioned` events added to `CircleEvent`
- `src/api/chat.rs` — `GET /api/chat?since=<ts>`, `POST /api/chat`, `GET /api/chat/stream` (SSE, chat events only)
- `src/commands/chat.rs` — `enoch chat [--follow] [--since=<ts>]`
- `src/commands/say.rs` — `enoch say "<text>"`

**Tasks:**
- [x] Add `chat` Y.Array to control doc; define `ChatMessage` struct in `src/control/mod.rs`
- [x] `GET /api/chat` — read messages, optional `?since=<ts>` filter
- [x] `POST /api/chat` — append message, emit `CircleEvent::MessagePosted`
- [x] `@mention` parsing — emits `CircleEvent::AgentMentioned` per mention (agent wake signal)
- [x] SSE stream for chat (`GET /api/chat/stream` — chat events only)
- [x] `enoch chat` and `enoch chat --follow` commands
- [x] `enoch say "<text>"` shorthand command

---

### M10 — Frontend
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

---

### M10.5 — Structured Collaboration (Automerge)
**Status: Planned**

Yjs is the right CRDT for high-frequency text sequences (code files, chat). For sparse, structured data — kanban boards, canvas nodes, rich block documents — **Automerge** is the better fit: it is a JSON CRDT designed exactly for this shape of data.

**Why Automerge for these features:**
- Kanban cards, canvas elements, and doc blocks are objects with named fields, not character sequences
- Automerge natively represents maps, lists, and scalars with per-field CRDT semantics — concurrent edits to different fields of the same card merge cleanly
- The [`automerge-rs`](https://github.com/automerge/automerge) Rust crate has a stable API and mature binary encoding
- Automerge documents are saved as compact binary (not operation logs), keeping storage small for sparse data

**Planned feature areas:**

| Feature | CRDT | Rationale |
|---------|------|-----------|
| Code / text files | Yjs | High-frequency character sequences |
| Chat messages | Yjs | Append-only array, already implemented |
| **Kanban board** | Automerge | Cards are structured objects with status, assignee, description fields |
| **Canvas / whiteboard** | Automerge | Nodes have position, size, content — sparse concurrent edits |
| **Block documents** | Automerge | Block tree structure; concurrent paragraph edits don't conflict |

**Architecture:**
- Automerge documents live alongside Yjs docs in the control doc's blob store (or a dedicated `automerge/` store)
- The existing `/enochian/sync/1.0.0` stream protocol is extended to handle Automerge sync messages in addition to Yjs
- Frontend uses [`@automerge/automerge-repo`](https://github.com/automerge/automerge-repo) for reactive binding to React components

**Tasks (to be scheduled when these features are built):**
- [ ] `automerge` crate added as dependency
- [ ] Automerge document store (`store/automerge.rs`) — persist and restore Automerge binary snapshots
- [ ] Sync protocol extension — handle Automerge sync messages in the existing P2P stream handler
- [ ] Kanban board backend — `GET/POST/PATCH /api/kanban` backed by an Automerge doc
- [ ] Canvas backend — node positions and edges stored in an Automerge doc
- [ ] Frontend: `@automerge/automerge-repo` + React bindings for kanban and canvas views

---

### M11 — Access Revocation via MLS (RFC 9420)
**Status: Planned**

True revocation requires changing the group key when a member is removed. Rather than rolling a custom PSK rotation scheme, ENOCHIAN will adopt **IETF MLS (RFC 9420)** — the international standard for group key management in decentralised systems, implemented in Rust by [`openmls`](https://github.com/openmls/openmls).

**Why MLS instead of custom PSK rotation:**
- MLS TreeKEM is cryptographically proven and handles eviction, offline members, and key rotation as first-class operations
- Offline members receive KeyPackages when they reconnect — no coordination window, no requirement for all members to be online simultaneously
- Forward secrecy and post-compromise security are built in
- `openmls` is a production Rust crate implementing RFC 9420

**Architecture:**

The existing PSK (via `libp2p::pnet`) is kept as a coarse transport-layer admission gate — proving you know the circle secret at all. MLS operates above it at the application layer, encrypting the CRDT sync data and managing group keys.

```
TCP
└── pnet (PSK, XSalsa20)     ← coarse gate: "are you in this circle?"
    └── Noise (identity)     ← peer authentication
        └── MLS group key    ← fine gate: "are you still a member?"
            └── CRDT sync    ← content (workspace files, chat, tasks)
```

When a member is evicted:
1. Admin runs `enoch member remove <peer>`
2. MLS `Remove` proposal is committed — TreeKEM prunes the evicted node and derives a new epoch key
3. All remaining members receive the new epoch key (online peers immediately, offline peers via KeyPackage on reconnect)
4. The evicted peer's key material is cryptographically useless for all future epochs
5. Even if they rejoin with a new keypair, they don't have a valid MLS KeyPackage for the new epoch — connection is rejected at the application layer, not just the member list

**Tasks:**
- [ ] Add `openmls` and `openmls_rust_crypto` dependencies
- [ ] MLS group creation at `enoch init` — group state stored in circle config dir
- [ ] KeyPackage generation and distribution on `enoch enter` — joining peer uploads their KeyPackage
- [ ] Encrypt CRDT sync updates with the current MLS epoch key
- [ ] `enoch member remove` issues a MLS `Remove` + `Commit`, distributes new epoch to remaining members
- [ ] Offline KeyPackage store — members who were offline receive pending commits when they reconnect
- [ ] `enoch member add` issues a MLS `Add` proposal (replacing current auto-registration for new peers)
- [ ] Sync-level rejection — peers presenting an outdated epoch key are refused
- [ ] Existing PSK invite links remain valid for transport admission; MLS KeyPackage is the revocation gate
- [ ] Update `docs/security.md` with the MLS threat model

---

### M12 — Anchor Node & WAN  *(was M11)*
**Status: Planned**

An anchor node is a regular `enochd` peer running 24/7 on a VPS. It acts as relay, rendezvous point, and always-on presence for its circles — no special code, just a peer that's always reachable. Teams that need WAN connectivity or strong liveness guarantees deploy one; LAN-only teams don't need it.

**Design:**

- Anchor node runs `enochd` with `--anchor` flag, which enables circuit relay and rendezvous server behaviors
- Invite links can embed the anchor's multiaddr as a bootstrap hint (`--relay <multiaddr>`)
- Peers that can't reach each other directly (NAT, firewall) connect via the anchor as relay
- Anchor node is just a peer — it holds the PSK, syncs files, maintains presence like any other member

**Tasks:**
- [ ] `--anchor` flag for `enochd` — enables `libp2p::relay::Behaviour` (circuit relay server)
- [ ] Wire rendezvous server (`libp2p-rendezvous`) so anchor acts as meeting point for WAN peers
- [ ] `enoch anchor` convenience command — generates a config with `--anchor` and prints multiaddr
- [ ] `enoch invite --relay <multiaddr>` — embed relay address in invite URI for WAN circles
- [ ] `enoch enter` reads relay address from invite and adds as bootstrap peer
- [ ] Kademlia DHT enabled for WAN peer discovery (already in roadmap; anchor acts as bootstrap node)
- [ ] Document anchor node VPS setup (minimal: any Linux box with a stable IP and open port)

---

### M13 — Packaging & Distribution  *(was M12)*
**Status: Planned**

Ship `enochd` and `enoch` as ready-to-use binaries for all major platforms. Anchor nodes ship as a Docker image.

**Tasks:**
- [ ] GitHub Actions CI — build and test on Linux, macOS, Windows on every push
- [ ] Release workflow — on `git tag v*`, build release binaries for all platforms and upload to GitHub Releases
- [ ] macOS: universal binary (x86_64 + aarch64), `.tar.gz` archive; optional `.app` bundle + DMG
- [ ] Linux: static musl binary (x86_64 + aarch64), `.tar.gz`; optional `.deb` and `.rpm` packages
- [ ] Windows: `enoch.exe` + `enochd.exe`, zipped; optional NSIS installer
- [ ] Docker image for anchor node — `ghcr.io/enochian/enochd:latest`; `docker run` one-liner in docs
- [ ] `install.sh` / `install.ps1` quick-install scripts (download latest release binary, place in PATH)
- [ ] Homebrew formula for macOS/Linux
