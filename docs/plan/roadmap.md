# enoxian Roadmap

## What works today

| Feature | Notes |
|---------|-------|
| Circle creation — `enox init` | Generates keypair + PSK + admin keypair; prints invite link with embedded admin pubkey |
| Invite links — `enoxian://v1/<b64>` | No-quote shell-safe URI, expiry enforced, admin pubkey embedded |
| Join — `enox enter` | Saves config + admin pubkey, workspace created, conflict handling |
| Multi-circle daemon — `enoxd` | Loads all enabled circles at startup; one P2P swarm per circle; hot-reload every 10s; starts HTTP server even with zero circles |
| Circle lifecycle | `enox disable/enable/leave`; `POST /circles/<id>/stop|start`; per-circle `CancellationToken` |
| Workspace folders | `~/enoxian/<name>/` per circle, configurable via `--dir` |
| REST API | Tasks, locks, presence, members, events SSE, lifecycle, files |
| Yjs CRDT + file watcher | Local file changes sync into CRDT, broadcast to local WS clients |
| WebSocket Yjs sync | Local editor/agent clients can sync documents over WS |
| Name-based circle resolution | `--circle Work` resolves by exact name → prefix → UUID prefix |
| `enox` CLI | init, enter, invite, circles, status, who, tasks, task-create, claim, done, bind, release, watch, disable, enable, leave, member |
| PSK-enforced transport | `pnet` XSalsa20 applied at TCP layer — cross-circle connections rejected at handshake |
| Live P2P file sync | Bidirectional y-sync over libp2p streams; mDNS auto-discovery; new files sync without reconnect |
| File sync hardening | Startup preload; post-handshake full-state push; macOS atomic-save fix (`Name(Both)`); temp file filter; CRDT state persistence across restarts |
| File deletion propagation | `all_deletes` broadcast; P2P sync propagates file removal across peers |
| Self-write loop prevention | Shared per-path flags prevent flush_to_disk from triggering re-sync |
| P2P echo prevention | Updates applied from peers use `"p2p"` origin; observer skips forwarding them back |
| P2P awareness relay | Cursor/presence awareness bytes forwarded between WS clients and across P2P peers via `all_awareness_updates` broadcast |
| Live presence | Hostname-based agent_id (strips `.local`); 30s heartbeat; immediate OFFLINE on disconnect/shutdown; stale detection (>90s); `enox who` shows last-seen age |
| Admin & member management | Admin keypair signs member operations; daemon auto-signs when `admin.key` present; `enox member list/add/remove/promote/pending/approve/reject`; pending queue; ghost-entry cleanup on rejoin |
| MLS access revocation (RFC 9420) | Group created at init; KeyPackages on join; Welcome delivery; serial commit watcher; `remove_member` issues Remove commit + rotates PSK; evicted peer fails pnet handshake |
| Chat | `enox say` posts messages; `enox chat [--follow]` reads/streams; `@mention` emits `AgentMentioned` event as agent wake signal |
| Task SSE events from P2P | Control doc observer fires `TaskCreated/Claimed/Done` SSE events when task updates arrive via P2P sync |
| Web frontend | React SPA: circle selector, presence panel, file tree, collaborative CodeMirror editor, task queue, chat, member management; served from `enoxd` at `/app` |
| Remote cursors | Yjs awareness in CodeMirror shows live cursor position and agent name label per connected peer |
| `enox open` | Opens the circle UI in the default browser (`http://127.0.0.1:36521/app`) |
| Production build | `cargo build --release` automatically runs `npm run build` via `build.rs` |
| WAN / circuit relay | Every node is relay server + client; relay circuit addr auto-derived and embedded in invites; DCUtR hole-punching; non-blocking background relay/rendezvous resolution |
| Bootstrap rendezvous server | `enoxd --bootstrap` — QUIC rendezvous + relay for both-behind-NAT; no PSK; stable keypair |
| QUIC transport | PSK-free QUIC leg alongside PSK-TCP; circle members connect to bootstrap via QUIC |
| Auto-embedded invites | `enox invite` queries the daemon and auto-embeds peer addr, relay, and rendezvous — no manual flags; relay circuit addr as fallback when no direct IP available |
| P2P status in API | `GET /api/status` returns `p2p.peer_id`, `p2p.external_addrs`, `p2p.relay_addrs`, `p2p.rendezvous_addrs` |

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

**Anchor nodes** are regular `enoxd` peers that happen to run 24/7 on a VPS. They act as relay and always-on presence for their circles. No special code — just a peer that's always reachable. Teams that need WAN or strong liveness guarantees deploy one; LAN-only teams don't need it.

**Bootstrap servers** run `enoxd --bootstrap` and provide rendezvous + relay for any circle, without joining any of them. They hold no PSK and cannot read synced content. `enox.suzent.com` is the shared public bootstrap for teams where no member has a public IP. A single bootstrap server serves all circles simultaneously.

**Conflict model:** CRDT handles concurrent real-time edits perfectly. For offline edits (both peers disconnected simultaneously), conflict detection uses the persisted CRDT state as the common ancestor. If only one side diverged, the merge is clean. If both sides diverged, the loser's version is preserved as a conflict copy (`file.conflict.<peer>`) and the CRDT merge becomes the working file.

---

## Milestone plan

### M1 — Workspace folders
**Status: Complete**

Each circle has a named, visible workspace directory (`~/enoxian/<circle-name>/` by default).
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

`libp2p_stream` behaviour is wired into every circle swarm. On `ConnectionEstablished` (dialing side), a `/enoxian/sync/1.0.0` stream is opened. A deadlock-free handshake exchanges `SyncStep1`/`SyncStep2` for all currently-open docs. After handshake, local updates are forwarded via the `all_updates` global broadcast channel; incoming updates are applied to the local CRDT and flushed to workspace disk. Verified end-to-end bidirectional file sync on a real LAN (Windows ↔ Mac).

**Implementation:**
- `src/network/sync.rs` — full sync handler; deadlock-free 3-phase handshake; lag recovery via full-state resend
- `src/state.rs` — `all_updates` broadcast + `self_write_flags` shared between watcher and flush_to_disk; observer kept alive with `mem::forget`; `"p2p"` origin filter prevents echo
- `src/store/fs.rs` — `flush_to_disk` uses `state.self_write_flags` (no longer takes a separate flag arg)
- `src/sync_yjs/watcher.rs` — handles Windows rename-sequence creation events (`Name(To)`) in addition to standard data-modify events
- `libp2p-stream = "0.4.0-alpha"` added to dependencies

**Tasks:**
- [x] Add `libp2p_stream` behaviour for `/enoxian/sync/1.0.0`
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
- [x] `enoxd` skips disabled circles at startup
- [x] `enox disable` — set flag in config, call `/circles/<id>/stop`
- [x] `enox enable` — clear flag in config, call `/circles/<id>/start`
- [x] `enox leave [--yes]` — confirm prompt, delete config dir, call `/circles/<id>/stop`
- [x] `POST /circles/<id>/stop` API endpoint — cancels token, removes from DaemonState
- [x] `POST /circles/<id>/start` API endpoint — reloads config, spawns circle, inserts into DaemonState
- [x] `enox circles` output shows disabled circles with a `[paused]` marker

---

### M5 — Presence
**Status: Complete**

On startup, each daemon writes a `Presence` entry (`agent_id = <hostname>-shortpeerid`, status=online, last_seen=now) to the control doc's `presence` Y.Map. The agent ID prefix uses the system hostname (stripping `.local` on macOS), falling back to `enoxian_AGENT_ID` env var or `"device"`. A 30-second heartbeat task refreshes `last_seen`. On clean shutdown, `OFFLINE` is written before the swarm drops so connected peers receive it immediately. On abrupt disconnect, the observing peer writes `OFFLINE` for the lost peer when the last P2P connection closes (`ConnectionClosed { num_established: 0 }`).

**Implementation:**
- `src/presence.rs` — `local_agent_id()` (hostname → strip `.local` → peer suffix), `spawn_presence()`, heartbeat loop, `write_offline()`
- `src/lifecycle.rs` — `ConnectionClosed` handler calls `presence::write_offline` for the disconnected peer's agent_id; clean-shutdown path writes OFFLINE before breaking swarm loop
- `src/state.rs` — control doc observer wired into `all_updates`
- `src/commands/who.rs` — last-seen age display, stale detection

**Tasks:**
- [x] Write presence entry (agent ID, hostname, timestamp) on daemon start
- [x] Refresh presence heartbeat every 30s via a tokio interval task
- [x] `enox who` displays live agents with last-seen time
- [x] Hostname-based agent_id (strips `.local`/domain suffix); `enoxian_AGENT_ID` override
- [x] Immediate OFFLINE on clean shutdown (written before swarm drops)
- [x] Immediate OFFLINE on abrupt disconnect (written by observing peer on `ConnectionClosed`)

---

### M6 — Admin & member management

See [admin.md](admin.md) for the full design.

**Status: Complete**

Admin keypair is generated at `enox init` and stored in `admin.key`. The public key is embedded in invite URIs so joining peers save it automatically. Member operations (add, remove, promote, approve, reject) require an admin signature; the daemon auto-signs from `admin.key` when a request arrives from the frontend with no signature provided.

> ⚠ **Security note:** The member list is a directory, not an access gate. Removing a peer from the list stops them from auto-registering on restart but does not revoke their PSK — they can still connect and sync, or rejoin with a fresh keypair. True access revocation requires MLS epoch rotation (M11). See [security.md](../security.md) for the full threat model.

**Implementation:**
- `src/invite.rs` — extended binary format: optional admin pubkey appended as u16-length-prefixed bytes (backward-compatible)
- `src/commands/init.rs` — generates admin keypair; saves `admin.key`; embeds pubkey in initial invite
- `src/commands/invite.rs` — loads `admin.key` if present, embeds pubkey in invite
- `src/commands/enter.rs` — extracts `admin_pubkey_hex` from invite, saves to `config.toml`
- `src/api/members.rs` — `list_members`, `add_member`, `remove_member`, `promote_member`, `list_pending`, `approve_member`, `reject_member`; all mutating ops verify admin signature; `resolve_admin_sig()` auto-signs with local `admin.key` when frontend omits signature
- `src/commands/member.rs` — `enox member list/add/remove/promote/pending/approve/reject`; auto-signs with `admin.key`
- `src/control/mod.rs` — `MemberEntry`, `MemberRole`, `PendingEntry`, `MEMBER_LIST_KEY`, `MLS_PENDING_KEY`, `MemberAdded`/`MemberRemoved` events
- `src/lifecycle.rs` — auto-registration on startup; stale pending cleanup on restart (already-registered path); ghost-entry eviction (same `agent_id`, different `peer_id` — leave/rejoin); non-admin member-list observer removes own pending on P2P approval; admin self-evict observer removes own pending if synced from remote

**Tasks:**
- [x] Admin keypair generated at `enox init`, stored as `admin.key`
- [x] Invite format carries admin pubkey (backward-compatible extension)
- [x] `enox enter` saves `admin_pubkey_hex` from invite to `config.toml`
- [x] Member list stored in control doc CRDT (`member_list` Y.Map) — replicated to all peers
- [x] `enox member list` — show all members and their roles
- [x] `enox member add <peer-id> [--role admin|member]` — admin-signed add
- [x] `enox member remove <peer-id>` — admin-signed remove
- [x] `enox member promote <peer-id>` — promote to admin
- [x] Daemon verifies admin signature on all member write operations
- [x] Auto-registration — each peer writes its own member entry to the CRDT on daemon start (role=Admin if `admin.key` present, Member otherwise); skips if entry already exists so explicit removals persist across restarts
- [x] Peer ID prefix/suffix resolution in `member remove` and `member promote`
- [x] Pending queue — non-admins write a `PendingEntry` on first join; `enox member pending` lists them
- [x] `enox member approve/reject <peer-id>` — admin approves or rejects pending entries
- [x] Frontend approve/reject UI — admin sees pending queue with APPROVE/REJECT buttons; device name shown
- [x] Daemon auto-sign — API handlers call `resolve_admin_sig()` so frontend can approve without shipping the private key to the browser
- [x] Stale pending cleanup — on restart, already-registered peers remove their own stale pending entry
- [x] Ghost-entry eviction — on rejoin (new keypair, same device), stale member entries matching the same `agent_id` are removed before inserting the new entry
- [x] Non-admin approval observer — member-list Yrs observer removes own pending entry when admin writes their member entry via P2P sync
- [x] Admin self-evict observer — admin's own pending entry (if any) is removed immediately, both at startup and on P2P arrival
- [x] Device name shown in member and pending lists alongside owner name

---

### M7 — CLI completeness
**Status: Complete**

**Implementation:**
- `src/commands/tasks.rs` — `create()` function posts to `POST /api/tasks`
- `src/cli.rs` — `TaskCreate { title, description }` subcommand
- `src/commands/serve.rs` — hot-reload loop (10s poll, shared with M4)

**Tasks:**
- [x] `enox task-create <title> [--description "..."]`
- [x] Hot-reload new circles without restarting `enoxd`

---

### M8 — File sync hardening
**Status: Complete**

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
- `src/store/fs.rs` — `flush_to_disk` saves CRDT state synchronously after file write (fixes race condition)
- `src/store/conflicts.rs` — `conflict_rel_path()`, `scan_conflicts()` (workspace walk for `*.conflict.*` files)
- `src/sync_yjs/watcher.rs` — startup preload, offline-edit detection, macOS `Name(Both)` fix, temp file filter; `.conflict.` files ignored
- `src/network/sync.rs` — post-handshake full-state push; file deletion propagation; `sv_has_divergence()` + `write_conflict_copy()`; pre-merge snapshot; conflict detection in both initiator and responder handshake paths
- `src/state.rs` — `all_deletes` broadcast for P2P file deletion; `all_awareness_updates` for P2P cursor relay
- `src/api/status.rs` — `/status` includes `conflicts: [...]` array from workspace scan
- `src/commands/status.rs` — `enox status` prints conflict list

**Tasks:**
- [x] CRDT state persistence across restarts (`.enoch_crdt/`)
- [x] Startup workspace preload
- [x] Post-handshake full-state push
- [x] macOS atomic-save fix (`Name(Both)` rename event)
- [x] Temp/hidden file filter
- [x] File deletion propagation via P2P (`all_deletes` broadcast + P2P sync handler)
- [x] Fix CRDT/file race condition — `crdt::save` now awaited synchronously in `flush_to_disk` (not background spawn)
- [x] Session ID — `store/session.rs`; incremented on every daemon start, stored in `~/.enoxian/circles/<id>/session_id`; held in `AppState`
- [x] `last_connected_at` — recorded per-peer on every sync handshake in `~/.enoxian/circles/<id>/peers/<peer_id>`
- [x] Exchange session metadata on reconnect — `\0session` frame exchanged before CRDT handshake; both sides log each other's session ID
- [x] Conflict detection — state vector divergence check (`sv_has_divergence`) in P2P handshake; both initiator and responder paths covered using pre-merge snapshot
- [x] Conflict copy — `<file>.conflict.<agent_id>` written before CRDT merge is applied; watcher ignores these files; `store/conflicts.rs`
- [x] `enox status` shows unresolved conflict files in the workspace (scanned from workspace dir; `/status` API + CLI display)

**Planned: Binary file dual-track sync**

Text files sync via Yjs (sequence CRDT). Binary and large files need a different path: content-addressed blob sync layered on top of the same P2P transport.

```
Text/code files   → Yjs CRDT (operation-level merge, real-time collaboration)
Binary/large files → Blob sync (hash ref in Yjs CRDT → fetch content by hash via libp2p)
```

Design:
- On write: hash the binary file (BLAKE3) → store as `.enoch_blobs/<hash>` → write the hash string into the Yjs doc for that path
- On sync: peer receives a hash ref → checks if it has the blob locally → if not, fetches via a new `/enoxian/blob/1.0.0` libp2p stream protocol
- The Yjs CRDT remains the source of truth for "which version is current" (the hash); the blob store is a content-addressed cache
- Large text files (> configurable threshold, default 1 MB) can also go through the blob path to avoid bloating CRDT state

Tasks (to be scheduled as part of M8 or a dedicated M8.5):
- [ ] BLAKE3 blob store (`store/blobs.rs`) — `put(bytes) → hash`, `get(hash) → bytes`, stored in `.enoch_blobs/`
- [ ] Binary/large file detection in watcher — route to blob store instead of Yjs text encoding
- [ ] Hash ref format in Yjs — store `blob:<hash>` as the doc content for binary paths
- [ ] `/enoxian/blob/1.0.0` stream protocol — request/response for blob fetch by hash
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
| `enox chat` | Print recent messages (last 50) |
| `enox chat --follow` | Stream new messages as they arrive (SSE) |
| `enox say "<text>"` | Post a message |

**Implementation:**
- `src/control/mod.rs` — `ChatMessage` struct (`id`, `agent_id`, `text`, `mentions`, `ts`); `CHAT_KEY` constant; `MessagePosted` and `AgentMentioned` events added to `CircleEvent`
- `src/api/chat.rs` — `GET /api/chat?since=<ts>`, `POST /api/chat`, `GET /api/chat/stream` (SSE, chat events only)
- `src/commands/chat.rs` — `enox chat [--follow] [--since=<ts>]`
- `src/commands/say.rs` — `enox say "<text>"`

**Tasks:**
- [x] Add `chat` Y.Array to control doc; define `ChatMessage` struct in `src/control/mod.rs`
- [x] `GET /api/chat` — read messages, optional `?since=<ts>` filter
- [x] `POST /api/chat` — append message, emit `CircleEvent::MessagePosted`
- [x] `@mention` parsing — emits `CircleEvent::AgentMentioned` per mention (agent wake signal)
- [x] SSE stream for chat (`GET /api/chat/stream` — chat events only)
- [x] `enox chat` and `enox chat --follow` commands
- [x] `enox say "<text>"` shorthand command

---

### M10 — Frontend
**Status: Complete**

A minimal web UI served by `enoxd` itself (no separate build server). Targets local agent use: one browser tab per circle, showing files, tasks, presence, and chat.

**Tech stack:**
- Vite + React + TypeScript
- [y-codemirror.next](https://github.com/yjs/y-codemirror.next) for collaborative editing (connects to `/ws/yjs`)
- `yjs` + `y-protocols` for local CRDT binding
- Tailwind CSS for styling

**Implementation (done):**
- `frontend/` — full Vite + React scaffold with Tailwind
- `src/api/files.rs` — `GET /api/files` reads `state.docs` keys (always current, avoids filesystem scan race)
- `frontend/src/components/EditorPanel.tsx` — CodeMirror 6 + `yCollab(ytext, awareness)`; custom `enochTheme`; remote cursor CSS
- `frontend/src/lib/YjsProvider.ts` — custom y-protocols provider with WS reconnect; deferred `connect()` to fix initial awareness race; `YjsConnectionStatus` ('connecting'|'synced'|'disconnected') callback
- `frontend/src/lib/agentColor.ts` — deterministic per-agent color from agent ID hash (7-color palette)
- `frontend/src/lib/constrainCursorLabels.ts` — CodeMirror `ViewPlugin` that repositions `.cm-ySelectionInfo` labels via `getBoundingClientRect()` clamping so they stay within the scroller on all browsers and cursor positions
- `frontend/src/lib/textBoundRemoteSelections.ts` — CodeMirror extension that prevents remote selection highlights from leaking outside the actual text content area
- `frontend/src/components/RightPanel.tsx` — file tree, task queue (create/claim/done), presence list, chat
- `frontend/src/components/ChatPanel.tsx` — scrolling message log + send box; SSE-backed live updates
- `frontend/src/context/AppContext.tsx` — active circle, daemon status, SSE connection
- `src/state.rs` — `awareness_updates` DashMap (atomic `entry().or_insert_with()`); `all_awareness_updates` broadcast for P2P relay

**Tasks:**
- [x] `frontend/` Vite + React scaffold; `npm run build` outputs to `static/`
- [x] Circle selector — `GET /circles` to list active circles
- [x] Presence panel — SSE-backed live presence; stale detection
- [x] Task panel — list, create, claim, done; live updates via SSE
- [x] Chat panel — load history + SSE stream; send via `POST /api/chat`
- [x] File tree — `GET /api/files` reads from `state.docs`
- [x] Collaborative editor — CodeMirror 6 + y-codemirror + `YjsProvider` bound to `/ws/yjs`
- [x] Remote cursors — Yjs awareness relayed between WS clients and across P2P peers; name labels clamped inside scroller via `constrainCursorLabels` ViewPlugin (all browsers/positions); remote selections clipped to text bounds
- [x] Connection status — `YjsConnectionStatus` ('connecting'|'synced'|'disconnected') shown in editor header
- [x] Agent colors — deterministic per-agent palette, shared between editor cursors and presence panel
- [x] `enoxd` serves `static/` at `/app` via `tower-http::ServeDir` (dev: Vite proxy only)
- [x] Production build step — `build.rs` runs `npm run build` automatically on `cargo build --release`
- [x] `enox open` — opens `http://127.0.0.1:36521/app` in the default browser

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
- The existing `/enoxian/sync/1.0.0` stream protocol is extended to handle Automerge sync messages in addition to Yjs
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
**Status: Complete**

True revocation requires changing the group key when a member is removed. enoxian uses **IETF MLS (RFC 9420)** — the international standard for group key management in decentralised systems, implemented in Rust by [`openmls`](https://github.com/openmls/openmls).

**Why MLS instead of custom PSK rotation:**
- MLS TreeKEM is cryptographically proven and handles eviction, offline members, and key rotation as first-class operations
- Offline members receive pending commits when they reconnect — no coordination window, no requirement for all members to be online simultaneously
- Forward secrecy and post-compromise security are built in
- `openmls` is a production Rust crate implementing RFC 9420

**Architecture:**

The existing PSK (via `libp2p::pnet`) is kept as a coarse transport-layer admission gate. MLS manages group key material above it and drives PSK rotation: each MLS epoch produces a new PSK that the evicted peer cannot derive.

```
TCP
└── pnet (PSK, XSalsa20)     ← coarse gate: "are you in this circle?"
    └── Noise (identity)     ← peer authentication
        └── sync gate        ← tombstone check: "were you explicitly removed?"
            └── MLS epoch → PSK  ← key gate: "do you have the current epoch key?"
                └── CRDT sync    ← content (workspace files, chat, tasks)
```

When a member is evicted:
1. Admin runs `enox member remove <peer>`
2. MLS `Remove` + `Commit` issued — TreeKEM prunes the evicted leaf, derives new epoch key
3. Remove commit broadcast via `mls_commits` CRDT array to all remaining members
4. Admin and each remaining peer apply the commit, derive the new PSK (`export_secret("enoxian-psk")`), and restart their swarm
5. Evicted peer's old PSK fails the pnet XSalsa20 handshake — connection refused before any data
6. Offline members apply the pending commit on reconnect and rotate their PSK

**Implementation:**
- `src/mls/group.rs` — `MlsGroupManager`: `create`, `join_from_welcome`, `add_member`, `remove_member`, `apply_commit`, `epoch_psk`, `leaf_index_for_peer`, `save`/`load`
- `src/mls/identity.rs` — `MlsIdentity`: Ed25519-based credential, signer, provider; persisted in circle config dir
- `src/lifecycle.rs` — admin bootstraps MLS group on startup; KeyPackage written to `mls_key_packages`; Welcome consumer observer; **serial commit watcher** (mpsc channel + single consumer task — prevents race conditions on MLS mutex when multiple commits arrive in one P2P batch); `rotate_psk_and_restart` rotates `config.psk_hex` and restarts the circle swarm
- `src/api/members.rs` — `approve_member`: load KeyPackage → `group.add_member()` → distribute commit + Welcome; `remove_member`: `group.remove_member()` → distribute Remove commit → writes tombstone → `rotate_psk_and_restart` in background task; cleans up key package, welcome, and pending entries
- `src/network/sync.rs` — membership gate at the top of `sync_inner`: checks `mls_removed` tombstone; rejects evicted peers before any CRDT data is exchanged, even during the brief window between member removal and PSK rotation completing
- `src/control/mod.rs` — `MLS_REMOVED_KEY`: `Map[peer_id → RFC-3339 timestamp]` — CRDT-replicated tombstone set

**Tasks:**
- [x] Add `openmls` and `openmls_rust_crypto` dependencies
- [x] MLS group creation at `enox init` — group state stored in circle config dir
- [x] `MlsIdentity` generation on `enox enter` — persisted in `~/.enoxian/circles/<id>/`
- [x] KeyPackage written to `mls_key_packages` Y.Map on daemon start
- [x] `approve_member` API: load KeyPackage → `group.add_member()` → distribute commit + Welcome via Yjs
- [x] Welcome consumer — watches `mls_welcomes` for own peer_id and applies Welcome to join MLS group
- [x] Commit watcher — serial mpsc consumer applies incoming MLS commits; rotates PSK after each epoch advance
- [x] Pending member queue with approve/reject workflow wired into MLS
- [x] `remove_member` API: `group.remove_member()` → Remove commit broadcast → `rotate_psk_and_restart`; evicts all auxiliary CRDT keys; writes peer offline
- [x] `rotate_psk_and_restart` — saves new PSK to config, stops circle, restarts with new pnet key
- [x] Serial commit processing — `mpsc::unbounded_channel` serialises concurrent commit arrivals; prevents MLS mutex races and double-PSK-rotation on batch sync
- [x] Sync-level tombstone gate — `mls_removed` CRDT map; `remove_member` writes peer_id tombstone; `sync_inner` rejects tombstoned peers before any data exchange; closes the PSK-rotation window
- [x] `docs/security.md` updated with MLS threat model, TreeKEM explanation, epoch → PSK derivation chain, commit propagation, tombstone gate design, and attacker capability tables

---

### M12 — WAN Support (Circuit Relay + DCUtR + Bootstrap Rendezvous)
**Status: Complete**

Every `enoxd` node includes both a circuit relay server and relay client. Any node with a public IP can serve as relay. Peers behind NAT connect through a relay and DCUtR attempts a direct hole-punch. For the case where **no** peer has a public IP, `enoxd --bootstrap` provides a public rendezvous + relay server.

**Design:**

- Every `enoxd` is simultaneously a relay server (can be used by others) and a relay client (can connect through others)
- Invite links auto-embed relay, rendezvous, and peer addresses — `enox invite` queries the daemon; no manual flags needed
- Relay and rendezvous addresses propagate: once one member joins with a relay/rendezvous, future invites they generate include it automatically
- The bootstrap server (`enoxd --bootstrap`) speaks QUIC only (no PSK) — it is not a circle member and cannot read any content
- Circle members connect to the bootstrap server via a separate QUIC transport leg (no PSK); direct circle-to-circle connections remain PSK-protected TCP

**Transport stack (circle swarms):**

```
TCP + PSK (XSalsa20) + Noise + Yamux   →  /ip4/.../tcp/...          (circle peers)
Circuit relay + Noise + Yamux           →  /ip4/.../tcp/.../p2p-circuit  (NAT traversal)
QUIC (no PSK)                           →  /ip4/.../udp/.../quic-v1  (bootstrap server)
```

**Implementation:**
- `Cargo.toml` — added `relay`, `dcutr`, `quic` to libp2p features
- `src/network/behaviour.rs` — `relay_client::Behaviour`, `relay::Behaviour`, `dcutr::Behaviour`, `rendezvous::client::Behaviour` in `EnochBehaviour`
- `src/network/bootstrap_behaviour.rs` — `BootstrapBehaviour`: `rendezvous::server::Behaviour` + `relay::Behaviour` + identify + ping + kad
- `src/bootstrap.rs` — `run_bootstrap(port)`: generates/loads stable keypair at `~/.enoxian/bootstrap.key`; QUIC listener; logs full multiaddr at startup
- `src/config.rs` — `relay_addrs` and `rendezvous_addrs: Vec<String>` (serde-defaulted)
- `src/state.rs` — `peer_id: String` and `p2p_external_addrs: Arc<RwLock<Vec<String>>>` added to `AppState`
- `src/invite.rs` — `relay_addr` and `rendezvous_addr` extensions in invite binary format (backward-compatible)
- `src/cli.rs` — `--relay`, `--rendezvous` flags on `InviteArgs`; `--bootstrap` flag on `DaemonCli`
- `src/commands/invite.rs` — queries `GET /api/status` for live P2P info; auto-embeds peer/relay/rendezvous; explicit flags override
- `src/commands/enter.rs` — saves `relay_addrs` and `rendezvous_addrs` from invite to `config.toml`
- `src/api/status.rs` — `GET /api/status` returns `p2p` section: peer_id, external_addrs, relay_addrs, rendezvous_addrs
- `src/lifecycle.rs` — QUIC transport wired alongside TCP+PSK; dials rendezvous servers on startup; registers namespace on connect; discovers peers; re-registers hourly; `ExternalAddrConfirmed` → `p2p_external_addrs`
- `src/bin/enoxd.rs` — `--bootstrap` flag routes to `bootstrap::run(port)`

**Tasks:**
- [x] Add `relay`, `dcutr`, `quic` libp2p features
- [x] `relay_client`, `relay`, `dcutr`, `rendezvous::client` behaviours in `EnochBehaviour`
- [x] QUIC transport (no PSK) wired alongside TCP+PSK in lifecycle.rs
- [x] `relay_addrs` and `rendezvous_addrs` in `CircleConfig`
- [x] Extension 2 (`relay_addr`) and extension 3 (`rendezvous_addr`) in invite binary format
- [x] `enox invite` auto-embeds peer/relay/rendezvous from daemon state and config; explicit flags override
- [x] `enox enter` saves relay and rendezvous addresses to config
- [x] Relay transport wired alongside TCP+PSK; `swarm.listen_on(<relay>/p2p-circuit)` on startup
- [x] DCUtR — direct hole-punch after relay connection
- [x] Rendezvous client — register + discover on connect; re-register every hour
- [x] `enoxd --bootstrap` — stable keypair; QUIC rendezvous server + relay server; no PSK; no circles
- [x] `peer_id` and `p2p_external_addrs` in AppState; populated by `ExternalAddrConfirmed` events
- [x] `GET /api/status` returns full `p2p` block
- [x] Non-blocking background rendezvous + relay resolution — default server resolved in background task, injected into running swarm via mpsc channel; never blocks `spawn_circle`
- [x] Relay circuit addr as peer_addr fallback in invites — when no external/direct addr available, derive `relay/p2p-circuit/p2p/MY_PEER_ID` from keypair + relay addr
- [x] `api/enter` fix — `no_verify: true` skips 10s connectivity swarm when called from HTTP handler; `tokio::spawn` isolates panics as `JoinError`
- [x] Daemon starts HTTP server even with zero circles (removed bail on empty config)

---

### M13 — Packaging & Distribution  *(was M12)*
**Status: Planned**

Ship `enoxd` and `enox` as ready-to-use binaries for all major platforms. Anchor nodes ship as a Docker image.

**Tasks:**
- [ ] GitHub Actions CI — build and test on Linux, macOS, Windows on every push
- [ ] Release workflow — on `git tag v*`, build release binaries for all platforms and upload to GitHub Releases
- [ ] macOS: universal binary (x86_64 + aarch64), `.tar.gz` archive; optional `.app` bundle + DMG
- [ ] Linux: static musl binary (x86_64 + aarch64), `.tar.gz`; optional `.deb` and `.rpm` packages
- [ ] Windows: `enox.exe` + `enoxd.exe`, zipped; optional NSIS installer
- [ ] Docker image for anchor node — `ghcr.io/enoxian/enoxd:latest`; `docker run` one-liner in docs
- [ ] `install.sh` / `install.ps1` quick-install scripts (download latest release binary, place in PATH)
- [ ] Homebrew formula for macOS/Linux
