# Getting Started

## Prerequisites

- Rust 1.83+
- Cargo

## Build

```bash
git clone <repo>
cd enochian
cargo build
```

Binaries are placed at:

```
target/debug/enochd    # daemon
target/debug/enoch     # agent CLI
```

For production use:

```bash
cargo build --release
# target/release/enochd  target/release/enoch
```

---

## Step 1 — Create a Circle

A Circle is the shared workspace. Run this once on any machine:

```bash
./target/debug/enoch init --name "MyCircle"
```

```
✦ Circle cast: MyCircle
  circle-id : 8e563c41-f0ec-4225-9764-064f1fb04341
  peer-id   : 12D3KooW...

  invite    : enochian://v1/CRxkUjpN...?expires=2026-05-20T14:00:00Z&name=MyCircle

  Share the invite link to let peers join (valid for 7d).
  Generate a new link anytime: enoch invite 8e563c41-...
```

**Save the invite link** — share it with peers over a trusted channel. It encodes both the circle ID and the secret key, and expires after 7 days by default.

---

## Step 2 — Start the Daemon

```bash
# bash / MSYS2
RUST_LOG=info ./target/debug/enochd serve --circle 8e563c41-f0ec-4225-9764-064f1fb04341

# PowerShell
$env:RUST_LOG = "info"
.\target\debug\enochd.exe serve --circle 8e563c41-f0ec-4225-9764-064f1fb04341
```

Expected output:

```
INFO  Starting enochd for circle 'MyCircle' (8e563c41-...)
INFO  PeerID:   12D3KooW...
INFO  SyncDir:  ~/.enochian/circles/8e563c41-.../files
INFO  HTTP/WS listening on :9090  (P2P on :9091)
INFO  P2P listening on /ip4/192.168.1.x/tcp/9091
```

---

## Step 3 — Point the CLI at the Daemon

Open a second terminal and set the API target:

```bash
# bash
export ENOCHIAN_API=http://127.0.0.1:9090/api

# PowerShell
$env:ENOCHIAN_API = "http://127.0.0.1:9090/api"
```

---

## Step 4 — Basic Commands

```bash
# Circle overview
enoch status

# Create a task (via REST — CLI has no `tasks create` subcommand yet)
curl -X POST http://127.0.0.1:9090/api/tasks \
  -H "Content-Type: application/json" \
  -d '{"title":"Write integration tests","created_by":"agent-alpha"}'

# List tasks
enoch tasks

# Claim a task
curl -X POST http://127.0.0.1:9090/api/claim \
  -H "Content-Type: application/json" \
  -d '{"task_id":"<id>","agent_id":"agent-alpha"}'

# Mark done
enoch done <task-id>

# Acquire a file lock
curl -X POST http://127.0.0.1:9090/api/bind \
  -H "Content-Type: application/json" \
  -d '{"path":"src/main.rs","agent_id":"agent-alpha"}'

# Watch live events in a separate terminal
enoch watch
```

---

## Step 5 — Second Agent (same LAN)

On another machine (or terminal), join using the invite link:

```bash
enoch enter "enochian://v1/CRxkUjpN...?expires=...&name=MyCircle"
```

mDNS discovers the daemon automatically on the same network. For WAN connections, generate an invite with the peer address embedded:

```bash
# On the host machine — embed your public P2P address
enoch invite <circle-id> --peer /ip4/1.2.3.4/tcp/9091

# Share the resulting enochian:// link — the invitee dials directly
enoch enter "enochian://v1/..."
```

Invites expire. Generate a fresh one anytime:

```bash
enoch invite <circle-id> --ttl 24h
```

---

## Next Steps

- [concepts.md](concepts.md) — understand Circles, Documents, and the Control Doc
- [cli.md](cli.md) — full command reference
- [api.md](api.md) — REST API for agent automation
