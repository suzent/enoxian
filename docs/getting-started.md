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
  workspace : /Users/suzy/enochian/MyCircle

  invite    : enochian://v1/CRxkUjpNaBcDeFgH...

  Share the invite link to let peers join (valid for 7d).
  Generate a new link anytime: enoch invite "MyCircle"
```

The workspace directory (`~/enochian/MyCircle`) is created automatically — this is where shared files live. The invite link encodes the circle ID and secret key. Share it over a trusted channel.

To use a different location:

```bash
enoch init --name "MyCircle" --dir ~/projects/myapp
```

---

## Step 2 — Start the Daemon

`enochd` loads **all** circles automatically from `~/.enochian/circles/`. Just start it:

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
INFO    Circle 'MyCircle' (8e563c41-...) — PeerID: 12D3KooW... — Workspace: /Users/suzy/enochian/MyCircle
INFO  HTTP/WS listening on :9090
INFO  [8e563c41-...] P2P listening on /ip4/192.168.1.x/tcp/<random>
```

All circles share one HTTP port. Each gets its own P2P swarm on a random port.

> **After any `cargo build`** restart `enochd` to pick up the new binary.

---

## Step 3 — Use the CLI

Open a second terminal. With one circle, `enoch` selects it automatically:

```bash
./target/debug/enoch status
```

```
◆ Circle:    MyCircle
  ID:        8e563c41-...
  Workspace: /Users/suzy/enochian/MyCircle
  Docs:      0
```

With multiple circles, specify by name:

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

# Create a task (via REST)
curl -X POST http://127.0.0.1:9090/circles/8e563c41-.../api/tasks \
  -H "Content-Type: application/json" \
  -d '{"title":"Write integration tests","created_by":"agent-alpha"}'

# List tasks
enoch tasks

# Claim and complete a task
enoch claim <task-id>
enoch done <task-id>

# Acquire and release a file lock (path relative to workspace)
enoch bind src/main.rs
enoch release src/main.rs

# Watch live events
enoch watch
```

---

## Step 5 — Second Agent (same LAN)

On another machine, join using the invite link (no quotes needed):

```bash
enoch enter enochian://v1/CRxkUjpNaBcDeFgH...
```

```
✦ Joining circle: MyCircle (8e563c41-...)
  Workspace : /Users/bob/enochian/MyCircle
  Config    → ~/.enochian/circles/8e563c41-.../config.toml
  ✦ Verified peer 12D3KooW... via /ip4/192.168.1.192/tcp/4494

  Start the daemon: enochd
  Then: enoch --circle "MyCircle" status
```

Then start `enochd` on the second machine — it picks up the saved circle and connects via mDNS automatically.

**Name conflict:** if you already have a local circle named `MyCircle` with a different ID, the workspace is auto-disambiguated:
```
⚠ A circle named 'MyCircle' already exists locally.
  Workspace → /Users/bob/enochian/MyCircle-d4e2e7
```

**Re-joining:** if you already have this exact circle, `enter` exits cleanly:
```
✦ Already a member of 'MyCircle' — nothing to do.
```

**WAN:** embed a peer address in the invite so the joiner can connect without mDNS:

```bash
enoch invite MyCircle --peer /ip4/1.2.3.4/tcp/9091
enoch enter enochian://v1/...
```

---

## Next Steps

- [concepts.md](concepts.md) — Circles, Documents, and the Control Doc
- [cli.md](cli.md) — full command reference
- [api.md](api.md) — REST API for agent automation
