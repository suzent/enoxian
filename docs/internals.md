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
                    ┌────────────-─────┐
                    │ broadcast::Sender│  raw v1 update bytes
                    └────────┬────-────┘
                             │
                    ┌────────┴────────┐
                    │  WS clients     │  Update message → each subscriber
                    └─────────────────┘

   WS Update arrives ───► apply_update() on Doc
                               │
                               ▼
                          flush_to_disk()  ───► write file
```

### Disk → CRDT (watcher)

`spawn_watcher()` starts a `notify::RecommendedWatcher` on the sync directory with `RecursiveMode::Recursive`. Events are bridged from `std::sync::mpsc` to `tokio::sync::mpsc` via a blocking thread.

On `Modify(Data)` or `Create(File)`:

1. Strip the sync directory prefix → relative path (forward-slash normalized).
2. Check the **self-write flag** for this path (see below). Skip if set.
3. Read the full file as UTF-8.
4. Call `state.get_or_create_doc(rel_path)` → `Arc<Doc>`.
5. Open a `TransactionMut`, get the `Y.Text`, compare with current content.
6. If changed: `remove_range(0, len)` then `insert(0, new_contents)`.
7. Drop the `TransactionMut` → `observe_update_v1` fires → broadcast channel receives raw bytes → WS clients receive `Update` messages.

**Current strategy: full-text replace.** This is correct for CRDT semantics — concurrent full-replaces merge deterministically. A future version will apply character-level diff operations for better performance on large files.

### CRDT → Disk (WebSocket update)

When a WS client sends `SyncStep2` or `Update`:

1. `Update::decode_v1(&bytes)` → `Update`
2. `doc.transact_mut().apply_update(update)` — merges into the Y.Doc; fires `observe_update_v1` → broadcasts to other WS clients.
3. `flush_to_disk()` — reads `Y.Text.get_string()` → writes the file.

### Self-write suppression

Without suppression, `flush_to_disk` would trigger the watcher, which would re-read the file and apply another update — infinite loop.

Suppression uses a per-path `Arc<AtomicBool>`:

```
flush_to_disk:
    flag.store(true)
    tokio::fs::write(file)

watcher (next event for this path):
    if flag.swap(false) { continue }   // skip — our own write
```

The flag map is a `DashMap<String, Arc<AtomicBool>>` shared between the watcher and `flush_to_disk`.

---

## P2P Layer

### Transport stack

```
TCP ──► Noise (XX handshake, Ed25519) ──► Yamux (multiplexer)
```

All connections are encrypted and authenticated. Noise uses the node's Ed25519 keypair (loaded from `config.toml`). Yamux allows multiple logical streams over one TCP connection.

### Behaviours

| Behaviour | Config | Purpose |
|-----------|--------|---------|
| `mdns` | default | LAN multicast discovery; fires `Discovered` / `Expired` events |
| `kad` | `Mode::Server` | Kademlia DHT for WAN routing; bootstraps from discovered peers |
| `identify` | protocol `/enochian/1.0.0` | Exchange public keys and listen addresses on connect |
| `ping` | default | Keepalive; detect dead connections |
| `rendezvous` (client) | — | Register with a rendezvous server for WAN introduction |

### Event loop

`serve.rs` runs the swarm in the second branch of `tokio::select!`:

```rust
tokio::select! {
    result = axum::serve(listener, app) => { ... }
    _ = async {
        loop {
            match swarm.select_next_some().await {
                NewListenAddr     => log address
                ConnectionEstablished => log peer
                Mdns::Discovered  => add to Kademlia, dial if not connected
                Mdns::Expired     => log
                Identify::Received => add listen addrs to Kademlia
                Ping              => debug log
                OutgoingConnectionError => warn
                _ => {}
            }
        }
    } => {}
}
```

### Y-doc gossip over P2P (planned)

Currently, Yjs document updates are only exchanged over the HTTP WebSocket endpoint. P2P gossip — broadcasting Y.Array / Y.Map / Y.Text updates directly between `enochd` nodes over libp2p streams — is planned for **Phase 4**. This will use `libp2p-request-response` or a custom stream protocol to sync the Control Doc and file docs between daemons without requiring a shared HTTP endpoint.
