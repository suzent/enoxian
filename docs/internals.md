# Internals

## Lock Arbitration

### Overview

File locks use a **Y.Array append-log** in the Control Document under the key `lock_log`. There is no lock server and no leader — every daemon that holds the same Y.Array state computes the same result deterministically.

### Log entries

Every acquire and release operation appends a JSON-encoded `LockEntry` to the array:

```json
{
  "entry_id": "uuid",
  "agent_id": "agent-alpha",
  "path":     "src/main.rs",
  "action":   "acquire",
  "ts":       "2026-05-13T14:00:00Z"
}
```

Entries are **immutable** once written. The array only grows.

### Arbitration algorithm

`compute_lock_state()` (`src/control/arbitration.rs`) replays the array from index 0:

```
holders = {}

for entry in lock_log:
    if entry.action == Acquire:
        if entry.path not in holders:          # first writer wins
            holders[entry.path] = entry.agent_id
    if entry.action == Release:
        if holders.get(entry.path) == entry.agent_id:
            del holders[entry.path]

return holders   # map of path → current holder
```

**First-writer-wins on concurrent acquires**: Yjs Array insertion order is stable under merge — whichever entry was inserted first (by vector-clock ordering) wins the slot. Both nodes see the same order after sync.

### Physical file protection

After a successful acquire, `set_readonly()` changes the file's permissions on disk:

- **Unix**: `chmod 444`
- **Windows**: `SetFileAttributes(FILE_ATTRIBUTE_READONLY)`

This prevents tools that don't speak ENOCHIAN from accidentally overwriting a locked file. The permission is restored on release.

---

## File Sync

### Overview

```
                    ┌─────────────────┐
   disk write  ───► │  notify watcher │
                    └────────┬────────┘
                             │  read file contents
                             ▼
                    ┌─────────────────┐
                    │  Y.Text update  │  (full-replace TransactionMut)
                    └────────┬────────┘
                             │  observe_update_v1 fires
                             ▼
                    ┌──────────────────┐
                    │ doc_updates      │  raw v1 bytes → WS clients
                    │ all_updates      │  (path, bytes) → P2P sync tasks
                    └────────┬─────────┘
                             │
               ┌─────────────┴─────────────┐
               │  WS clients               │  P2P peers
               └───────────────────────────┘

   WS or P2P update arrives ───► apply_update() on Doc
                                      │  (origin="p2p" skips all_updates)
                                      ▼
                                 flush_to_disk()  ───► write file
                                      │  (sets self_write_flag first)
                                      ▼
                              watcher sees event → flag set → skip
```

### Disk → CRDT (watcher)

`spawn_watcher()` starts a `notify::RecommendedWatcher` on the workspace directory with `RecursiveMode::Recursive`. Events are bridged from `std::sync::mpsc` to `tokio::sync::mpsc` via a blocking thread.

Handled event kinds: `Modify(Data(_))`, `Modify(Any)`, `Modify(Name(To))`, `Create(File)`, `Create(Any)`. The `Name(To)` variant is required on Windows, where file creation via Explorer generates a rename sequence rather than a direct create event.

On a relevant event:

1. Strip the workspace prefix → relative path (forward-slash normalized).
2. Check `state.self_write_flags` for this path. If set, swap to false and skip — this was our own `flush_to_disk` write.
3. Read the full file as UTF-8.
4. Call `state.get_or_create_doc(rel_path)` → `Arc<Doc>`.
5. Open a `TransactionMut`, get the `Y.Text`, compare with current content.
6. If changed: `remove_range(0, len)` then `insert(0, new_contents)`.
7. Drop the `TransactionMut` → `observe_update_v1` fires → `doc_updates` and `all_updates` broadcast channels receive the update.

**Current strategy: full-text replace.** Correct for CRDT semantics — concurrent full-replaces merge deterministically. A future version may apply character-level diffs for performance on large files.

### CRDT → Disk (WebSocket or P2P update)

When a WS client sends `SyncStep2` or `Update`, or when the P2P sync handler receives an update from a peer:

1. `Update::decode_v1(&bytes)` → `Update`
2. `doc.transact_mut().apply_update(update)` (or `transact_mut_with("p2p")` for P2P) — merges into the Y.Doc.
3. `flush_to_disk()` — reads `Y.Text.get_string()` → writes the file.

For WS updates, `observe_update_v1` fires and sends to both `doc_updates` (other WS clients) and `all_updates` (P2P peers). For P2P updates (origin `"p2p"`), only `doc_updates` fires — `all_updates` is skipped to prevent echo.

### Self-write suppression

Without suppression, `flush_to_disk` would trigger the watcher, which would re-read the file and apply another update — infinite loop.

Suppression uses a per-path `Arc<AtomicBool>` stored in `AppState::self_write_flags`, shared between the watcher and `flush_to_disk`:

```
flush_to_disk:
    flag = state.self_write_flags[path]
    flag.store(true)
    tokio::fs::write(file)

watcher (next event for this path):
    if flag.swap(false) { continue }   // skip — our own write
```

Storing the flags in `AppState` is critical — without a shared map, each caller would hold an independent `AtomicBool` and the handshake would never work.

---

## P2P Layer

### Transport stack

```
TCP ──► PSK (XSalsa20, pnet) ──► Noise (XX handshake, Ed25519) ──► Yamux (multiplexer)
```

The PSK layer is applied first via `pnet::PnetConfig` using `with_other_transport`. A node with a mismatched PSK is rejected before Noise negotiation begins — this is the circle membership enforcement at the network layer. Noise then authenticates the node's Ed25519 keypair. Yamux multiplexes logical streams over the single TCP connection.

### Behaviours

| Behaviour | Config | Purpose |
|-----------|--------|---------|
| `mdns` | default | LAN multicast discovery; fires `Discovered` / `Expired` events |
| `kad` | `Mode::Server` | Kademlia DHT for WAN routing; bootstraps from discovered peers |
| `identify` | protocol `/enochian/1.0.0` | Exchange public keys and listen addresses on connect |
| `ping` | default | Keepalive; detect dead connections |
| `rendezvous` (client) | — | Register with a rendezvous server for WAN introduction |
| `stream` (`libp2p-stream`) | — | Custom stream protocol `/enochian/sync/1.0.0` for y-sync |

### Event loop

`serve.rs` spawns a swarm event loop task per circle:

```rust
loop {
    match swarm.select_next_some().await {
        NewListenAddr          => log address
        ConnectionEstablished  => if dialer: open_stream → run_sync(initiator=true)
        ConnectionClosed       => log
        Mdns::Discovered       => add to Kademlia, dial (DisconnectedAndNotDialing)
        Mdns::Expired          => log
        Identify::Received     => add listen addrs to Kademlia
        Ping                   => debug log
        OutgoingConnectionError => debug log (harmless — already-connected peer)
        _ => {}
    }
}
```

A separate accept task runs per circle:

```rust
let mut incoming = stream_control.accept(sync::PROTOCOL)?;
while let Some((peer_id, stream)) = incoming.next().await {
    tokio::spawn(sync::run_sync(peer_id, stream, state, false));
}
```

Only the dialing side opens the sync stream (responder accepts) — this prevents a double-sync when both sides dial simultaneously.

### Y-sync protocol (`/enochian/sync/1.0.0`)

Live bidirectional file sync between daemons uses the y-sync protocol over a `libp2p-stream` `Stream`. Framing: `[4-byte path len][path UTF-8][4-byte data len][y-sync bytes]`.

**3-phase handshake (deadlock-free):**

```
Initiator                          Responder
─────────────────────────────────────────────
send count + SyncStep1 for each doc
                                   recv SyncStep1s → send SyncStep2s
                                   send count + SyncStep1 for each doc
recv SyncStep2s
recv SyncStep1s → send SyncStep2s
─────────────────────────────────────────────
         continuous update exchange
```

**Continuous exchange** (after handshake):

- A reader task runs in a dedicated `tokio::spawn` so `read_frame` is never cancelled mid-frame.
- Reader forwards events to the writer loop via an `mpsc` channel.
- Writer loop selects between incoming events and outgoing updates from `all_updates`.
- On `RecvError::Lagged`: sends full CRDT state for every doc (idempotent — CRDT merges are safe to re-apply).

**Echo prevention:** updates received from a peer are applied with `transact_mut_with("p2p")`. The `observe_update_v1` callback checks the origin and skips forwarding to `all_updates`, so the update is not echoed back to the sender.

**Self-write suppression:** `flush_to_disk` sets a per-path flag in `AppState::self_write_flags` before writing. The file watcher checks and clears the flag on the next event for that path, skipping re-ingestion of P2P-written files.

**Observer lifetime:** the `Subscription` from `observe_update_v1` is kept alive with `std::mem::forget`. Dropping it would silently unregister the callback in yrs 0.26.
