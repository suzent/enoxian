# Daemon Reference — `enox daemon run`

`enox daemon run` is the long-running mode of the unified `enox` binary. It
serves **all known Circles** over a single HTTP/WS port, with each Circle getting
its own Iroh endpoint on a random UDP port. Normal users should prefer `enox start` or
`enox service install`.

```
Usage: enox daemon run [OPTIONS]

Options:
  --port <PORT>    HTTP port [default: 36521]
  --bind-lan       Bind the API to 0.0.0.0 instead of loopback
  --bind <IP>      Explicit bind address (overrides --bind-lan)
  -h, --help
```

---

## Startup sequence

1. Scan `~/.enoxian/circles/*/config.toml` and load all known circles
2. Skip circles whose config has `disabled = true`
3. For each enabled circle:
   - Create in-memory state for documents, broadcasts, file-write suppression, and proposal sync
   - Spawn a file watcher on the circle's workspace directory
   - Bind an Iroh endpoint keyed by the Circle's Ed25519 key, homed on the Circle's relays
   - Accept incoming streams and start dialing members (see [Networking](#networking))
   - Register the circle in shared daemon state
4. Start a single HTTP/WS server on `--port` serving all circles

The circle PSK is a stable per-circle network credential: every stream starts
with a proof derived from it. Member removal is
enforced by replicated member/tombstone state and advances the MLS epoch so
removed members cannot derive new content-encryption keys.

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
| `POST /circles/<id>/api/unclaim` | Return a claimed task to the open pool |
| `POST /circles/<id>/api/done` | Mark task done |
| `POST /circles/<id>/api/bind` | Acquire file lock |
| `POST /circles/<id>/api/actors/register` | Issue a short-lived device-bound actor token |
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
force_relay       = false                # use relay paths only (diagnostic)
peers             = []                   # peers named by the invite, dialed on join
rendezvous_addrs  = []                   # bootstrap server (short links, enox link, upgrade check)
iroh_relays       = []                   # Iroh relay URLs; empty = default relays
join_policy       = "auto"               # auto or manual
owner             = "alice"              # human/device owner label
```

Configs written by 0.11 or earlier may also have `relay_addrs` and
`transport`. Both still load and are no longer read.

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
RUST_LOG=debug enox daemon run          # full verbosity including Iroh internals
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

## Networking

Every Circle connects over Iroh (QUIC). 0.11 and earlier used libp2p; the two
cannot talk to each other, so every device in a Circle must run 0.12 or later.

**Endpoint.** Each Circle has its own Iroh endpoint, keyed by the Circle's
Ed25519 key (derived per Circle from the device identity). Its EndpointId is
the same public key as the member's PeerId, so the PeerId stays the member id.
Nothing is published to Iroh's DNS discovery, and port mapping (UPnP/NAT-PMP)
is off.

**Streams.** One connection per pair of peers carries every protocol under the
single ALPN `enoxian/3`. Each bidirectional stream opens with a one-byte
protocol tag (mls-bootstrap, admin-handover, sync, proposals, events), then a
32-byte Circle proof in each direction. The proof is an HKDF of the Circle PSK
bound to the Circle id and both peers' ids, so it cannot be replayed to another
peer, and it covers relayed paths too. Streams must show their tag and proof
within 10 seconds. Content protocols also require the peer to have a leaf in
this device's MLS group; mls-bootstrap needs only the proof, since it is how a
joiner gets its Welcome. A removed member's connection is closed at once, and
removed members are never dialed.

**Dialing.** Of two members, the one with the lower PeerId dials. A joiner also
dials the peers its invite named, since they do not know it yet. If two
connections to one peer appear, the one dialed by the lower PeerId stays. A
dial names the peer id, all of the Circle's relay URLs, and any direct-address
hint from an invite.

**Reconnects.** The daemon sweeps the member list every 30 seconds and redials
missing peers, with per-peer exponential backoff. After losing a peer it dials,
it tries again after 2 seconds. A connection with no traffic for 15 seconds is
dropped; Iroh sends keep-alives every 5 seconds.

**Paths.** Iroh holepunches and migrates paths inside one QUIC connection: a
connection that starts on a relay moves to a direct path when one opens,
without reconnecting. Behind a
symmetric NAT (a mobile carrier, say) no direct path opens and traffic stays on
the relay. The member list shows the selected path as LAN, Tailscale, Public
or Relay.

**Relays.** A Circle uses its config's `iroh_relays` when that list is not
empty. Otherwise it uses `https://relay.enoxian.com` plus Iroh's public relays.
Each device homes on the nearest one; other devices reach it there until a
direct path opens. Invites carry the inviter's `iroh_relays`, and `enox enter`
saves them into the joiner's config.

**Status.** `GET /circles/<id>/api/status` reports the endpoint under `p2p`:
`peer_id`, `endpoint_id`, `relays`, `home_relay`, `direct_addrs`,
`force_relay` and `recent_conn_errors`. See the
[API reference](api.md).

## Force-relay diagnostics

Each Circle has a device-local `force_relay` setting. In the frontend, open
**Settings → Connectivity** and enable **Force relay**. The daemon keeps running
while only that Circle is restarted. With force relay on, the Circle's endpoint
has no IP transports, so every connection goes through a relay and no direct
path is tried.

The setting is persisted in the Circle's `config.toml`. Disable the toggle to
return to automatic routing. A relayed member connection appears as `RELAY` in
the member list.

## Bootstrap mode

`enox bootstrap serve` runs the bootstrap server. It does not load circles,
holds no circle PSKs, and does no peer discovery.

```bash
enox bootstrap serve --port 36521
```

It serves HTTP on `--port`: `/version` (including `min_client_version`, so old
clients see an upgrade notice), `/peer-id`, `/pair` for `enox link`, and
`/invite` for short invites. Its stable keypair is stored at
`~/.enoxian/bootstrap.key`. With `--iroh-relay` (and `--advertise-host`) it
also runs an Iroh relay; see the [CLI reference](../../guide/cli.md#bootstrap-serve).

---

## Environment variables

| Variable | Effect |
|----------|--------|
| `RUST_LOG` | Tracing log filter |
| `ENOXIAN_AGENT_ID` | Local presence/agent ID prefix |
| `ENOXIAN_HOME` | Override the enoxian state directory (default `~/.enoxian`) |
| `ENOXIAN_API` | Base URL used by the `enox` CLI |
| `ENOXIAN_CIRCLE` | Default circle target used by the `enox` CLI |
| `ENOXIAN_SRC` | Source path used by `enox update --dev` |
