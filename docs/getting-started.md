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
  Generate a new link anytime: enoch invite MyCircle
```

**Save the invite link** — share it over a trusted channel. It encodes both the circle ID and the secret key, and expires after 7 days by default.

---

## Step 2 — Start the Daemon

`enochd` automatically loads **all** circles from `~/.enochian/circles/`. Just start it:

```bash
# bash / MSYS2
RUST_LOG=info ./target/debug/enochd

# PowerShell
$env:RUST_LOG = "info"
.\target\debug\enochd.exe
```

Expected output:

```
INFO  Starting enochd — 1 circle(s) found
INFO    Circle 'MyCircle' (8e563c41-...) — PeerID: 12D3KooW... — SyncDir: ~/.enochian/circles/.../files
INFO  HTTP/WS listening on :9090
INFO  [8e563c41-...] P2P listening on /ip4/192.168.1.x/tcp/<random>
```

The daemon serves all circles on port 9090. Each circle gets its own P2P swarm on a random port.

---

## Step 3 — Use the CLI

Open a second terminal. If you have only one circle, `enoch` selects it automatically:

```bash
./target/debug/enoch status
```

With multiple circles, specify one by name:

```bash
./target/debug/enoch --circle MyCircle status
# or via env var
export ENOCHIAN_CIRCLE=MyCircle
./target/debug/enoch status
```

List all known circles:

```bash
./target/debug/enoch circles
```

---

## Step 4 — Basic Commands

```bash
# Circle overview
enoch status

# Create a task
curl -X POST http://127.0.0.1:9090/circles/8e563c41-.../api/tasks \
  -H "Content-Type: application/json" \
  -d '{"title":"Write integration tests","created_by":"agent-alpha"}'

# List tasks
enoch tasks

# Claim a task
enoch claim <task-id>

# Mark done
enoch done <task-id>

# Acquire a file lock
enoch bind src/main.rs

# Watch live events
enoch watch
```

---

## Step 5 — Second Agent (same LAN)

On another machine (or terminal), join using the invite link:

```bash
enoch enter "enochian://v1/CRxkUjpN...?expires=...&name=MyCircle"
```

mDNS discovers the daemon automatically on the same network. For WAN, embed the peer address in the invite:

```bash
# On the host — embed your public P2P address
enoch invite MyCircle --peer /ip4/1.2.3.4/tcp/9091

# Share the link; the invitee dials directly
enoch enter "enochian://v1/..."
```

Generate fresh invites anytime:

```bash
enoch invite MyCircle --ttl 24h
```

---

## Next Steps

- [concepts.md](concepts.md) — Circles, Documents, and the Control Doc
- [cli.md](cli.md) — full command reference
- [api.md](api.md) — REST API for agent automation
