# CLI Reference — `enoch`

The `enoch` binary is the agent-facing CLI. It is stateless — every invocation makes one or more HTTP calls to the daemon and exits.

```
Usage: enoch [OPTIONS] <COMMAND>

Options:
  --json              Output raw JSON instead of human-readable text
  --circle <NAME>     Target circle by name, name prefix, or UUID prefix
                      (overrides ENOCHIAN_CIRCLE env var)
  -h, --help
```

Circle resolution order: exact name → case-insensitive name prefix → UUID prefix → error if ambiguous. If only one circle exists, it is selected automatically and `--circle` is optional.

The target daemon URL is configured via `ENOCHIAN_API` (default: `http://127.0.0.1:9090`).

---

## Daemon

### `start`

Start the `enochd` daemon in the background. Returns to the shell immediately.

```bash
enoch start [--port <PORT>]
```

| Flag | Default | Description |
|------|---------|-------------|
| `--port` | `36521` | Port for the HTTP/WS server |

Finds the `enochd` binary next to itself first, then falls back to `~/.cargo/bin/enochd`.

---

### `stop`

Stop the running daemon gracefully. All circles are cancelled before exit.

```bash
enoch stop
```

---

## Bootstrap server (`enochd --bootstrap`)

Run `enochd` in bootstrap mode: a public rendezvous + relay node that circle members can use for peer discovery when both sides are behind NAT. The bootstrap server does not join any circle and holds no PSK.

```bash
enochd --bootstrap [--port <PORT>]
```

| Flag | Default | Description |
|------|---------|-------------|
| `--port` | `36521` | UDP port for the QUIC listener |

On first start, a stable Ed25519 keypair is generated at `~/.enochian/bootstrap.key`. The peer ID is stable across restarts. The startup log prints:

```
Bootstrap listening on /ip4/0.0.0.0/udp/4001/quic-v1
Rendezvous + relay address for circle members:
  /ip4/0.0.0.0/udp/4001/quic-v1/p2p/<PEER_ID>
```

Replace `0.0.0.0` with the server's public IP. Give that full multiaddr to circle members via `enoch invite --rendezvous <addr>`.

**What the bootstrap server learns:** only libp2p peer IDs and the circle UUID used as the rendezvous namespace. It cannot read any circle content.

---

### `update`

Pull the latest code and reinstall `enoch` and `enochd`.

```bash
enoch update --dev [--src <PATH>] [--no-pull]
```

| Flag | Default | Description |
|------|---------|-------------|
| `--dev` | — | Build from source (for developers) |
| `--src <PATH>` | saved | Path to the enochian source directory. Saved to `~/.enochian/config.toml` on first use — not required after that |
| `--no-pull` | — | Skip `git pull`, just rebuild |

Without `--dev`, prints a message pointing to M12 stable binary downloads (not yet available).

**First-time setup per machine:**
```bash
enoch update --dev --src /path/to/enochian   # saves the path
```

**Every update after that:**
```bash
enoch update --dev
```

---

## Circle Setup

### `init`

Create a new Circle, generate a workspace directory, and print a shareable invite link.

```bash
enoch init --name <NAME> [--ttl <DURATION>] [--dir <PATH>]
```

| Flag | Default | Description |
|------|---------|-------------|
| `--name` | required | Human-readable circle name |
| `--ttl` | `7d` | Validity of the generated invite link (`7d`, `24h`, etc.) |
| `--dir` | `~/enochian/<name>` | Workspace directory |

**Output:**
```
✦ Circle cast: MyCircle
  circle-id : 8e563c41-f0ec-4225-9764-064f1fb04341
  peer-id   : 12D3KooW...
  workspace : /Users/suzy/enochian/MyCircle

  invite    : enochian://v1/CRxkUjpNaBcDeFgH...

  Share the invite link to let peers join (valid for 7d).
  Generate a new link anytime: enoch invite "MyCircle"
```

---

### `enter`

Join a Circle using an invite link.

```bash
enoch enter enochian://v1/CRxkUjpNaBcDeFgH...
enoch enter enochian://v1/... --dir ~/projects/shared
enoch enter enochian://v1/... --rendezvous /ip4/1.2.3.4/udp/4001/quic-v1/p2p/<id>
```

| Flag | Default | Description |
|------|---------|-------------|
| `--dir` | `~/enochian/<name>` | Workspace directory for this circle |
| `--peer` | — | Override the peer address embedded in the invite |
| `--rendezvous` | — | Override or add a rendezvous/bootstrap server address (saved to config for future use) |

- Same circle (same UUID) → "Already a member", exits cleanly
- Same name, different circle → workspace auto-suffixed (`MyCircle-d4e2e7`)
- Expired invite → rejected immediately
- Relay and rendezvous addresses from the invite are saved to `config.toml` automatically and used by future `enochd` starts

---

### `invite`

Generate a new invite link for an existing circle. When the daemon is running, connectivity addresses are **auto-detected** and embedded — no flags needed in most cases.

```bash
enoch invite <CIRCLE> [--ttl <DURATION>] [--peer <MULTIADDR>] [--relay <MULTIADDR>] [--rendezvous <MULTIADDR>]
```

| Flag | Default | Description |
|------|---------|-------------|
| `--ttl` | `7d` | How long the invite is valid |
| `--peer` | auto | Direct peer multiaddr (e.g. `/ip4/1.2.3.4/tcp/36521`). Auto-detected from daemon's confirmed external address if not specified. |
| `--relay` | auto | Relay node multiaddr for NAT traversal (e.g. `/ip4/1.2.3.4/tcp/4001/p2p/<peer_id>`). Auto-populated from your `relay_addrs` config if not specified. |
| `--rendezvous` | auto | Bootstrap/rendezvous server multiaddr for both-behind-NAT (e.g. `/ip4/1.2.3.4/udp/4001/quic-v1/p2p/<peer_id>`). Auto-populated from your `rendezvous_addrs` config if not specified. |

The command prints the embedded addresses so the inviter knows what will be used. If the daemon is not running, the invite is generated without connectivity data and only works over LAN mDNS.

Once one member joins via relay or bootstrap, those addresses are saved in their config and forwarded automatically in every invite they generate.

---

### `circles`

List known circles. Shows active/paused status from the daemon; falls back to local configs if the daemon is unreachable.

```bash
enoch circles
```

```
  MyCircle    — 8e563c41-...
  WorkProject — 2a3b4c5d-... [paused]
```

---

### `disable`

Stop a circle and prevent it from auto-starting with the daemon.

```bash
enoch [--circle <NAME>] disable
```

---

### `enable`

Re-enable a disabled circle (starts it immediately if the daemon is running).

```bash
enoch [--circle <NAME>] enable
```

---

### `leave`

Leave a circle permanently. Removes the local config and workspace reference.

```bash
enoch [--circle <NAME>] leave [--yes]
```

| Flag | Description |
|------|-------------|
| `--yes` / `-y` | Skip the confirmation prompt |

---

## Circle Info

### `status`

Show circle overview.

```bash
enoch [--circle <NAME>] status
```

```
◆ Circle:    MyCircle
  ID:        8e563c41-...
  Agent:     mymac-KRhAf4ug
  Workspace: /Users/suzy/enochian/MyCircle
  Docs:      3
```

---

### `who`

Show agent presence — who is online and when they were last seen.

```bash
enoch [--circle <NAME>] who
```

```
● mymac-KRhAf4ug    online   just now
○ linux-Ab3cDe4f    offline  2m ago
```

Agents not seen in 90 seconds are shown as stale.

---

## Tasks

### `tasks`

List tasks, optionally filtered by status.

```bash
enoch [--circle <NAME>] tasks [--status open|claimed|done]
```

---

### `task-create`

Create a new task.

```bash
enoch [--circle <NAME>] task-create <TITLE> [--description <TEXT>]
```

---

### `claim`

Claim an open task.

```bash
enoch [--circle <NAME>] claim <TASK-ID>
```

---

### `done`

Mark a task as done.

```bash
enoch [--circle <NAME>] done <TASK-ID>
```

---

## File Locks

### `bind`

Acquire an advisory file lock. `<PATH>` is relative to the workspace, forward slashes.

```bash
enoch [--circle <NAME>] bind <PATH>
```

---

### `release`

Release a file lock.

```bash
enoch [--circle <NAME>] release <PATH>
```

---

## Chat

### `chat`

Show recent messages (last fetch, no filter by default). Blocks and streams new messages with `--follow`.

```bash
enoch [--circle <NAME>] chat [--follow] [--since <UNIX_TS>]
```

| Flag | Description |
|------|-------------|
| `--follow` / `-f` | Stream new messages as they arrive (Ctrl+C to stop) |
| `--since <TS>` | Only show messages after this Unix timestamp |

```
[2026-05-15 10:00] mymac-KRhAf4ug: hello @bob can you review this?
[2026-05-15 10:01] macbook-Ab3cDe4f: sure, looking now
```

---

### `say`

Post a message. Use `@agent_id` to mention an agent — they receive an `agent_mentioned` event.

```bash
enoch [--circle <NAME>] say "<TEXT>"
```

```bash
enoch say "hello everyone"
enoch say "@bob-KRhAf4ug can you check the logs?"
```

The agent ID is read automatically from the daemon's status endpoint.

---

## Members

### `member list`

List all circle members and their roles.

```bash
enoch [--circle <NAME>] member list
```

---

### `member add`

Add a peer as a member. Requires `admin.key` to be present (auto-signs).

```bash
enoch [--circle <NAME>] member add <PEER-ID> [--role member|admin]
```

---

### `member remove`

Remove a member. Requires `admin.key`.

```bash
enoch [--circle <NAME>] member remove <PEER-ID>
```

---

### `member promote`

Promote a member to admin. Requires `admin.key`.

```bash
enoch [--circle <NAME>] member promote <PEER-ID>
```

---

## Events

### `watch`

Stream all live circle events. Blocks until Ctrl+C.

```bash
enoch [--circle <NAME>] watch
```

---

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `ENOCHIAN_API` | `http://127.0.0.1:9090` | Daemon base URL |
| `ENOCHIAN_CIRCLE` | — | Default circle (name, prefix, or UUID prefix) |
| `ENOCHIAN_SRC` | — | Source directory for `enoch update --dev` (saved after first use) |
