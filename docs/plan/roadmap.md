# ENOCHIAN Roadmap

## What works today (v0.4.0)

| Feature | Notes |
|---------|-------|
| Circle creation — `enoch init` | Generates keypair + PSK, prints invite link |
| Invite links — `enochian://v1/<b64>` | No-quote shell-safe URI, expiry enforced |
| Join — `enoch enter` | Saves config, workspace created, conflict handling, exits cleanly |
| Multi-circle daemon — `enochd` | Loads all circles at startup, one P2P swarm per circle; one machine can join multiple circles simultaneously |
| Workspace folders | `~/enochian/<name>/` per circle, configurable via `--dir` |
| REST API | Tasks, locks, presence (read), events SSE |
| Yjs CRDT + file watcher | Local file changes sync into CRDT, broadcast to local WS clients |
| WebSocket Yjs sync | Local editor/agent clients can sync documents over WS |
| Name-based circle resolution | `--circle Work` resolves by exact name → prefix → UUID prefix |
| `enoch` CLI | init, enter, invite, circles, status, who, tasks, claim, done, bind, release, watch |
| PSK-enforced transport | `pnet` XSalsa20 applied at TCP layer — cross-circle connections rejected at handshake |
| Live P2P file sync | Bidirectional y-sync over libp2p streams; mDNS auto-discovery; new files sync without reconnect |
| Admin keypair | Generated at `enoch init`; stored as `admin.key`; unenforced until M6 |
| Self-write loop prevention | Shared per-path flags prevent flush_to_disk from triggering re-sync |
| P2P echo prevention | Updates applied from peers use `"p2p"` origin; observer skips forwarding them back |

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
**Status: Planned**

Currently all circles load at daemon startup and run until the daemon is killed. There is no way to disable, leave, or toggle individual circles.

See [lifecycle.md](lifecycle.md) for the full design.



**Operations:**

| Command | Description |
|---------|-------------|
| `enoch disable <circle>` | Pause a circle — stop its P2P swarm and file watcher, keep config |
| `enoch enable <circle>` | Resume a disabled circle |
| `enoch leave <circle>` | Permanently remove a circle from this machine |

**Runtime control (no daemon restart):**

| Endpoint | Description |
|----------|-------------|
| `POST /circles/<id>/stop` | Stop a circle's swarm at runtime |
| `POST /circles/<id>/start` | Start a stopped circle at runtime |

**Tasks:**
- [ ] Add `disabled: bool` field to `CircleConfig` (default false, `#[serde(default)]`)
- [ ] `enochd` skips disabled circles at startup
- [ ] `enoch disable <circle>` — set flag in config, call `/circles/<id>/stop`
- [ ] `enoch enable <circle>` — clear flag in config, call `/circles/<id>/start`
- [ ] `enoch leave <circle>` — confirm prompt, delete config dir, call `/circles/<id>/stop`
- [ ] `POST /circles/<id>/stop` API endpoint — drop swarm task + watcher, remove from DaemonState
- [ ] `POST /circles/<id>/start` API endpoint — re-load config, spawn swarm + watcher, insert into DaemonState
- [ ] `enoch circles` output shows disabled circles with a `[paused]` marker

---

### M5 — Presence
**Status: Planned**

`enoch who` reads the presence map from the control doc but no code writes to it. Agents never announce themselves.

**Tasks:**
- [ ] Write presence entry (agent ID, hostname, timestamp) on daemon start
- [ ] Refresh presence heartbeat every 30s via a tokio interval task
- [ ] `enoch who` displays live agents with last-seen time

---

### M6 — Admin & member management

See [admin.md](admin.md) for the full design.

**Status: Planned**

> ⚠ **Security prerequisite.** The current shared-PSK model has no access control — anyone with a valid invite link is a permanent equal member with no way to be removed. This milestone is required before ENOCHIAN is safe for any multi-user or production use.

**The problem with shared PSK:**
- Any peer can generate invites — there is no invite gating
- Revoking a member requires rotating the PSK and manually redistributing it to every remaining member out-of-band
- The CRDT merges all writes equally — there is no concept of read-only or restricted members

**Required architecture change — per-member credentials:**

| Component | Current | With admin |
|-----------|---------|------------|
| Membership credential | Shared PSK (everyone equal) | Admin-signed member list (per-member public key) |
| Invite authority | Any peer | Admin keypair only |
| Revocation | Impossible without PSK rotation | Admin removes key from member list |
| Write permissions | All peers equal | Tiered: admin / member / observer |

The PSK becomes a transport-layer network filter only ("can you reach the swarm"). Authorization moves to a signed member list stored in the control doc.

**Tasks:**
- [ ] Design signed member list format (admin keypair signs `{peer_id, role, added_at}` entries)
- [ ] `enoch init` designates the creator as admin (admin keypair stored separately from node keypair)
- [ ] Invites signed by admin key — peers verify signature on `enoch enter`
- [ ] Member list stored in control doc CRDT — replicated to all peers
- [ ] `enoch member list` — show all members and their roles
- [ ] `enoch member remove <peer-id>` — admin removes member, broadcasts updated list
- [ ] Peers reject connections from removed members (check member list on connect)
- [ ] `enoch member add-admin <peer-id>` — promote a member to admin

---

### M7 — CLI completeness
**Status: Planned**

**Tasks:**
- [ ] `enoch task create --title "..." [--description "..."]`
- [ ] Hot-reload new circles without restarting `enochd` (watch `~/.enochian/circles/`)
