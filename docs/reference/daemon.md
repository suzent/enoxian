# Daemon Reference — `enox daemon run`

`enox daemon run` is the long-running mode of the unified `enox` binary. It
serves **all known Circles** over a single HTTP/WS port, with each Circle getting
its own P2P swarm on a random port. Normal users should prefer `enox start` or
`enox service install`.

```
Usage: enox daemon run [OPTIONS]

Options:
  --port <PORT>    HTTP port [default: 36521]
  --bootstrap      Run as a public rendezvous + relay server, not as a circle daemon
  -h, --help
```

---

## Startup sequence

1. Scan `~/.enoxian/circles/*/config.toml` and load all known circles
2. Skip circles whose config has `disabled = true`
3. For each enabled circle:
   - Create in-memory state for documents, broadcasts, file-write suppression, and proposal sync
   - Spawn a file watcher on the circle's workspace directory
   - Build a libp2p swarm with Noise/Yamux, mDNS, Kademlia, Identify, Ping, Rendezvous, Relay/DCUtR, and stream protocols
   - Dial configured peers, relay addresses, and rendezvous servers
   - Spawn the membership bootstrap and encrypted `/enoxian/sync/2.0.0` stream accept tasks
   - Spawn the swarm event loop and register the circle in shared daemon state
4. Start a single HTTP/WS server on `--port` serving all circles

The circle PSK is a stable per-circle network credential. Member removal is
enforced by the replicated member/tombstone state; content-layer cryptographic
revocation is still future work.

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
| `GET /circles/<id>/api/files` | List tracked files |
| `POST /circles/<id>/api/files/create` | Create a file |
| `POST /circles/<id>/api/files/rename` | Rename a file |
| `POST /circles/<id>/api/files/delete` | Delete a file |
| `GET /circles/<id>/api/chat` | Read chat |
| `POST /circles/<id>/api/chat` | Post chat |
| `GET /circles/<id>/api/proposals` | List proposals |
| `GET /circles/<id>/api/proposals/<proposal_id>` | Show proposal details |
| `POST /circles/<id>/api/proposals/<proposal_id>/accept` | Accept proposal |
| `POST /circles/<id>/api/proposals/<proposal_id>/reject` | Reject proposal |
| `POST /circles/<id>/api/proposals/<proposal_id>/revert` | Revert proposal |
| `GET /circles/<id>/members` | List members |
| `GET /circles/<id>/members/pending` | List pending join requests |
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
admin_pubkey_hex  = "0803..."            # Ed25519 admin pubkey
disabled          = false                # skip this circle on daemon startup
peers             = []                   # direct peer multiaddrs to dial
relay_addrs       = []                   # circuit relay multiaddrs
rendezvous_addrs  = []                   # QUIC rendezvous server multiaddrs
join_policy       = "auto"               # auto or manual
owner             = "alice"              # human/device owner label
```

> Do not share `keypair_proto_hex`. The `psk_hex` is the circle network
> credential and is embedded in invite links.

The `admin_pubkey_hex` is generated at `enox init`; the private admin key lives
in `admin.key` alongside `config.toml` on admin machines. Member API operations
require admin signatures, and the CLI signs automatically when `admin.key` is
present.

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
RUST_LOG=info  enox daemon run          # recommended for normal use
RUST_LOG=debug enox daemon run          # full verbosity including libp2p internals
RUST_LOG=warn  enox daemon run          # errors and warnings only
```

---

## Local API security

The HTTP/WS API is a **privileged control plane** — it can add agents, arm
push-mode (letting a chat mention run a process), start/stop circles, and edit
config. It is not a public endpoint. Three defenses guard it:

**Loopback by default.** The daemon binds `127.0.0.1` only, so nothing off-host can
reach it. Opt into wider exposure explicitly:

```bash
enox daemon run                       # 127.0.0.1 (default)
enox daemon run --bind-lan            # 0.0.0.0 — reachable on the LAN
enox daemon run --bind 192.168.1.5    # a specific interface
```

**Token auth.** Every API request must present a token (generated on first start,
stored at `~/.enoxian/api.token`, owner-readable). Missing/wrong token → `401`.

- The `enox` CLI reads the file and sends `Authorization: Bearer <token>`.
- The frontend receives the token injected into its served HTML
  (`window.__ENOX_TOKEN__`); a cross-origin page cannot read that response, so it
  cannot steal the token. WebSocket/SSE connections (which cannot set headers)
  pass it as `?token=<token>`.

**CORS allowlist.** Only local origins (`localhost`, `127.0.0.1`, `[::1]`) may
make cross-origin requests. A permissive policy would let any website's scripts
read authenticated responses from this control plane.

**Safe remote access.** Do **not** expose the API directly to the internet.
Prefer tunnelling loopback to the remote machine:

```bash
ssh -L 36521:127.0.0.1:36521 user@host   # then use http://127.0.0.1:36521 locally
```

`--bind-lan` is acceptable only on a network you fully trust; the token is still
required, but widening the bind widens the attack surface.

## Force-relay diagnostics

Each Circle has a device-local `force_relay` setting. In the frontend, open
**Settings → Connectivity** and enable **Force relay**. The daemon keeps running
while only that Circle's P2P swarm is rebuilt. Force-relay mode:

- disables direct TCP and QUIC listeners
- skips saved direct-peer addresses
- disables DCUtR direct upgrades
- ignores non-circuit addresses returned by rendezvous
- rejects incoming direct circle-peer connections

The setting is persisted in the Circle's `config.toml`. Disable the toggle to
return to automatic routing. A successful relayed member connection appears as
`RELAY` in the member list and contains `/p2p-circuit` in its address.

## Bootstrap mode

`enox bootstrap serve` runs a public rendezvous + circuit relay server. It does not
load circles and holds no circle PSKs.

```bash
enox bootstrap serve --port 36521
```

The server listens over QUIC for libp2p rendezvous/relay traffic and exposes
`GET /peer-id` over HTTP on the same port so `enox invite --rendezvous <host>`
can resolve the server's peer ID. Its stable keypair is stored at
`~/.enoxian/bootstrap.key`.

---

## Environment variables

| Variable | Effect |
|----------|--------|
| `RUST_LOG` | Tracing log filter |
| `ENOXIAN_AGENT_ID` | Local presence/agent ID prefix |
| `ENOXIAN_API` | Base URL used by the `enox` CLI |
| `ENOXIAN_CIRCLE` | Default circle target used by the `enox` CLI |
| `ENOXIAN_SRC` | Source path used by `enox update --dev` |
