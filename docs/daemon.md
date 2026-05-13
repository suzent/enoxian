# Daemon Reference — `enochd`

The `enochd` binary is the long-running daemon. It serves **all known Circles** over a single HTTP/WS port, with each Circle getting its own P2P swarm on a random port.

```
Usage: enochd [OPTIONS]

Options:
  --port <PORT>    HTTP port [default: 9090]
  -h, --help
```

---

## Startup sequence

1. Scan `~/.enochian/circles/*/config.toml` and load all known circles
2. For each circle:
   a. Create in-memory AppState (CRDT doc store)
   b. Spawn a file watcher on `~/.enochian/circles/<id>/files`
   c. Build a libp2p swarm (TCP + Noise + Yamux + mDNS + Kademlia + Identify + Ping + Rendezvous) on a random port
   d. Register the circle in the shared DaemonState
3. Start a single HTTP/WS server on `--port` serving all circles

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

Located at `~/.enochian/circles/<circle-id>/config.toml`. Created by `enoch init`.

```toml
circle_id         = "8e563c41-f0ec-4225-9764-064f1fb04341"
circle_name       = "MyCircle"
psk_hex           = "d2d89de6..."        # 256-bit pre-shared key
keypair_proto_hex = "0802..."            # Ed25519 keypair, protobuf-encoded hex
```

> Do not share `keypair_proto_hex`. The `psk_hex` is the membership credential.

---

## Sync directory

`~/.enochian/circles/<circle-id>/files` — one per circle, fixed.

Files are watched recursively. Any write triggers a Y.Text CRDT update and broadcasts to connected WebSocket clients.

---

## Log levels

```bash
RUST_LOG=info  enochd          # recommended for normal use
RUST_LOG=debug enochd          # full verbosity including libp2p internals
RUST_LOG=warn  enochd          # errors and warnings only
```

---

## Environment variables

| Variable | Effect |
|----------|--------|
| `RUST_LOG` | Tracing log filter |
