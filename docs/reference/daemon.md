# Daemon Reference — `enoxd`

The `enoxd` binary is the long-running daemon. It serves **all known Circles** over a single HTTP/WS port, with each Circle getting its own P2P swarm on a random port.

```
Usage: enoxd [OPTIONS]

Options:
  --port <PORT>    HTTP port [default: 36521]
  -h, --help
```

---

## Startup sequence

1. Scan `~/.enoxian/circles/*/config.toml` and load all known circles
2. For each circle:
   a. Create in-memory state (CRDT doc store, `all_updates` broadcast, `self_write_flags`)
   b. Spawn a file watcher on the circle's workspace directory
   c. Build a libp2p swarm with PSK-enforced transport (XSalsa20 via `pnet`) + Noise + Yamux + mDNS + Kademlia + Identify + Ping + Rendezvous + Stream, on a random port
   d. Spawn the stream accept task (listens for incoming `/enoxian/sync/1.0.0` streams)
   e. Spawn the swarm event loop (dials mDNS peers, opens sync streams on connect)
   f. Register the circle in the shared daemon state
3. Start a single HTTP/WS server on `--port` serving all circles

Each circle's P2P swarm is isolated by its PSK — peers from a different circle are rejected at the transport layer before any protocol negotiation.

---

## API routing

All per-circle endpoints are prefixed with `/circles/<circle-id>`:

| Path | Description |
|------|-------------|
| `GET /circles` | List all active circles |
| `GET /circles/<id>/api/status` | Circle status |
| `GET /circles/<id>/api/who` | Agent presence |
| `GET /circles/<id>/api/tasks` | Task list |
| `POST /circles/<id>/api/tasks` | Create task |
| `POST /circles/<id>/api/claim` | Claim task |
| `POST /circles/<id>/api/done` | Mark task done |
| `POST /circles/<id>/api/bind` | Acquire file lock |
| `POST /circles/<id>/api/release` | Release file lock |
| `GET /circles/<id>/api/events` | SSE event stream |
| `GET /circles/<id>/ws/yjs?path=<file>` | Yjs WebSocket sync |

---

## Configuration file

Located at `~/.enoxian/circles/<circle-id>/config.toml`. Created by `enox init` or `enox enter`.

```toml
circle_id         = "8e563c41-f0ec-4225-9764-064f1fb04341"
circle_name       = "MyCircle"
psk_hex           = "d2d89de6..."        # 256-bit pre-shared key (circle membership)
keypair_proto_hex = "0802..."            # Ed25519 node keypair, protobuf-encoded hex
workspace_dir     = "/Users/suzy/enoxian/MyCircle"
admin_pubkey_hex  = "0803..."            # Ed25519 admin pubkey (enforced in M6)
```

> Do not share `keypair_proto_hex`. The `psk_hex` is the circle membership credential — every member holds it and any member can generate invite links (until M6 restricts invite authority to the admin keypair).

The `admin_pubkey_hex` is generated at `enox init` and stored in `admin.key` (private) alongside `config.toml`. It is replicated into joining members' configs via the invite flow. It is currently stored but not enforced — enforcement of invite signing and member lists is planned for M6.

---

## Workspace directory

Each circle has a **workspace** — a visible directory where shared files live.

| Scenario | Default location |
|----------|-----------------|
| `enox init --name MyCircle` | `~/enoxian/MyCircle` |
| `enox init --name MyCircle --dir ~/projects` | `~/projects` |
| `enox enter <invite>` | `~/enoxian/<circle-name>` |
| Name conflict on join | `~/enoxian/<circle-name>-<short-id>` |
| Old config without `workspace_dir` | `~/.enoxian/circles/<id>/files` (migration fallback) |

Files in the workspace are watched recursively. Any write triggers a CRDT update and broadcasts to connected WebSocket clients.

---

## Log levels

```bash
RUST_LOG=info  enoxd          # recommended for normal use
RUST_LOG=debug enoxd          # full verbosity including libp2p internals
RUST_LOG=warn  enoxd          # errors and warnings only
```

---

## Environment variables

| Variable | Effect |
|----------|--------|
| `RUST_LOG` | Tracing log filter |
