# Daemon Reference — `enochd`

The `enochd` binary is the long-running daemon. It hosts the P2P node, HTTP/WS API server, and file watcher for a single Circle.

```
Usage: enochd serve [OPTIONS] --circle <CIRCLE>

Options:
  --circle <CIRCLE>      Circle ID (UUID)                          [required]
  --port <PORT>          HTTP port                                 [default: 9090]
  --sync-dir <PATH>      Override the sync directory
  -h, --help
```

---

## Ports

The daemon uses two ports:

| Port | Role |
|------|------|
| `--port` (default 9090) | HTTP REST API + WebSocket sync |
| `--port + 1` (default 9091) | libp2p P2P TCP transport |

These are always separated by 1 so they don't conflict.

---

## Startup sequence

1. Load `~/.enochian/circles/<circle-id>/config.toml`
2. Build the libp2p swarm (TCP + Noise + Yamux + mDNS + Kademlia + Identify + Ping + Rendezvous)
3. Start file watcher on the sync directory (`notify`)
4. Bind HTTP/WS server on `--port`
5. Begin P2P listen on `--port + 1`
6. Run swarm event loop and HTTP server concurrently via `tokio::select!`

---

## Configuration file

Located at `~/.enochian/circles/<circle-id>/config.toml`. Created by `enoch init`.

```toml
circle_id         = "8e563c41-f0ec-4225-9764-064f1fb04341"
circle_name       = "MyCircle"
psk_hex           = "d2d89de6..."        # 256-bit pre-shared key
keypair_proto_hex = "0802..."            # Ed25519 keypair, protobuf-encoded hex
```

> Do not share `keypair_proto_hex`. The `secret` (`psk_hex`) is the membership credential.

---

## Log levels

Controlled by the `RUST_LOG` environment variable:

```bash
RUST_LOG=info  enochd serve ...      # recommended for normal use
RUST_LOG=debug enochd serve ...      # full verbosity including libp2p internals
RUST_LOG=enochian=debug enochd ...   # only enochian crate logs
RUST_LOG=warn  enochd serve ...      # errors and warnings only (silent operation)
```

---

## Sync directory

Default: `~/.enochian/circles/<circle-id>/files`

Override: `--sync-dir /path/to/dir`

All files under this directory are watched recursively. Any file write triggers a Y.Text CRDT update and broadcasts the change to connected WebSocket clients.

---

## Environment variables

| Variable | Effect |
|----------|--------|
| `RUST_LOG` | Tracing log filter |
