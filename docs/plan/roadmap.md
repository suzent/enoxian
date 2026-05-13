# ENOCHIAN Roadmap

## What works today (v0.2.0)

| Feature | Notes |
|---------|-------|
| Circle creation — `enoch init` | Generates keypair + PSK, prints invite link |
| Invite links — `enochian://v1/<b64>` | No-quote shell-safe URI, expiry enforced |
| Join — `enoch enter` | Saves config, verifies connectivity, exits cleanly |
| Multi-circle daemon — `enochd` | Loads all circles at startup, one P2P swarm per circle |
| REST API | Tasks, locks, presence (read), events SSE |
| Yjs CRDT + file watcher | Local file changes sync into CRDT, broadcast to local WS clients |
| WebSocket Yjs sync | Local editor/agent clients can sync documents over WS |
| Name-based circle resolution | `--circle Work` resolves by exact name → prefix → UUID prefix |
| `enoch` CLI | init, enter, invite, circles, status, who, tasks, claim, done, bind, release, watch |

---

## Milestone plan

### M1 — Workspace folders
**Status: Planned**

Each circle has a named, visible workspace directory (`~/enochian/<circle-name>/` by default). This replaces the current hidden `~/.enochian/circles/<id>/files/` sync directory.

See [workspace.md](workspace.md) for the full design and implementation plan.

---

### M2 — Secure network (PSK enforcement)
**Status: Planned**

The pre-shared key is saved in `config.toml` but never applied to the libp2p swarm. Any node can connect to any circle's P2P swarm — there is no membership check at the transport layer.

Fix: apply `pnet::PnetConfig` (with the circle PSK) to each swarm via `.with_other_transport()` in the SwarmBuilder. Nodes with a different PSK fail the handshake before Noise even starts.

**Tasks:**
- [ ] Apply circle PSK to swarm in `commands/serve.rs`
- [ ] Apply circle PSK to swarm in `commands/enter.rs` (connectivity check)
- [ ] Verify that cross-circle connections are rejected

---

### M3 — Live P2P sync (core protocol)
**Status: Planned**

This is the defining feature of ENOCHIAN. The libp2p swarm connects peers and the Yjs CRDT stores state locally — but they don't talk to each other. When two `enochd` instances connect, no document state is exchanged.

What needs to happen on peer connect:
1. Open a libp2p stream on `/enochian/sync/1.0.0`
2. Perform y-sync handshake (SyncStep1 → SyncStep2) for the control doc
3. Subscribe to local CRDT updates and forward them to the remote peer
4. Apply incoming remote updates to local CRDT and flush to disk

This reuses the y-sync protocol already in `sync_yjs/ws_handler.rs` — lifted into a libp2p stream handler.

**Tasks:**
- [ ] Add `request_response` or `libp2p_stream` behaviour for `/enochian/sync/1.0.0`
- [ ] On `ConnectionEstablished`: open sync stream, run y-sync handshake for control doc
- [ ] Subscribe to `doc_updates` broadcast channel per doc, forward to peer stream
- [ ] On incoming update: apply to local CRDT, flush to workspace disk
- [ ] Handle multi-doc sync (sync additional docs as they are opened)

---

### M4 — Presence
**Status: Planned**

`enoch who` reads the presence map from the control doc but no code writes to it. Agents never announce themselves.

**Tasks:**
- [ ] Write presence entry (agent ID, hostname, timestamp) on daemon start
- [ ] Refresh presence heartbeat every 30s via a tokio interval task
- [ ] `enoch who` displays live agents with last-seen time

---

### M5 — CLI completeness
**Status: Planned**

**Tasks:**
- [ ] `enoch task create --title "..." [--description "..."]`
- [ ] Hot-reload new circles without restarting `enochd` (watch `~/.enochian/circles/`)
