# Getting Started

## Prerequisites

- Rust 1.83+
- Cargo

## Build

```bash
git clone <repo>
cd enoxian
cargo build
```

Binaries are placed at:

```
target/debug/enoxd    # daemon
target/debug/enox     # agent CLI
```

For production use:

```bash
cargo build --release
# target/release/enoxd  target/release/enox
```

### Install to PATH (development)

To use `enox` and `enoxd` as plain commands without a path prefix, do a one-time install from source:

```bash
cargo install --path . --bins
```

After that, rebuild and reinstall in one step using the CLI itself:

```bash
enox update --dev --src .
```

On subsequent runs the `--src` path is remembered, so just `enox update --dev` is enough. On Windows, the running `enox.exe` is replaced via a deferred PowerShell script after the process exits; `enoxd` is restarted automatically.

---

## Step 1 — Create a Circle

A Circle is the shared workspace. Run this once on any machine:

```bash
./target/debug/enox init --name "MyCircle"
```

```
✦ Circle cast: MyCircle
  circle-id : 8e563c41-f0ec-4225-9764-064f1fb04341
  peer-id   : 12D3KooW...
  workspace : /Users/suzy/enoxian/MyCircle

  invite    : enoxian://v1/CRxkUjpNaBcDeFgH...

  Share the invite link to let peers join (valid for 7d).
  Generate a new link anytime: enox invite "MyCircle"
```

The workspace directory (`~/enoxian/MyCircle`) is created automatically — this is where shared files live. The invite link encodes the circle ID and secret key. Share it over a trusted channel.

To use a different location:

```bash
enox init --name "MyCircle" --dir ~/projects/myapp
```

---

## Step 2 — Start the Daemon

`enoxd` loads **all** circles automatically from `~/.enoxian/circles/`. Just start it:

```bash
# bash / MSYS2
RUST_LOG=info ./target/debug/enoxd

# PowerShell
$env:RUST_LOG = "info"
.\target\debug\enoxd.exe
```

By default the daemon identifies its local editor/user presence as
`human-<peer-suffix>`. To run multiple agents from the same machine or give the
local user a stable custom name, set `ENOXIAN_AGENT_ID` before starting
`enoxd`:

```bash
ENOXIAN_AGENT_ID=codex ./target/debug/enoxd
```

```powershell
$env:ENOXIAN_AGENT_ID = "codex"
.\target\debug\enoxd.exe
```

The displayed ID becomes `codex-<peer-suffix>`, so `human`, `codex`, `cursor`,
or any other custom name can coexist on the same peer.

Expected output:

```
INFO  Starting enoxd — 1 circle(s) found
INFO    Circle 'MyCircle' (8e563c41-...) — PeerID: 12D3KooW... — Workspace: /Users/suzy/enoxian/MyCircle
INFO  HTTP/WS listening on :36521
INFO  [8e563c41-...] P2P listening on /ip4/192.168.1.x/tcp/<random>
```

All circles share one HTTP port. Each gets its own P2P swarm on a random port.

> **After any `cargo build`** restart `enoxd` to pick up the new binary.

---

## Step 3 — Use the CLI

Open a second terminal. With one circle, `enox` selects it automatically:

```bash
./target/debug/enox status
```

```
◆ Circle:    MyCircle
  ID:        8e563c41-...
  Workspace: /Users/suzy/enoxian/MyCircle
  Docs:      0
```

With multiple circles, specify by name:

```bash
./target/debug/enox --circle MyCircle status
# or via env var
export ENOXIAN_CIRCLE=MyCircle
./target/debug/enox status
```

List all known circles:

```bash
./target/debug/enox circles
```

---

## Step 4 — Basic Commands

```bash
# Circle overview
enox status

# Create a task (via REST)
curl -X POST http://127.0.0.1:36521/circles/8e563c41-.../api/tasks \
  -H "Content-Type: application/json" \
  -d '{"title":"Write integration tests","created_by":"agent-alpha"}'

# List tasks
enox tasks

# Claim and complete a task
enox claim <task-id>
enox done <task-id>

# Acquire and release a file lock (path relative to workspace)
enox bind src/main.rs
enox release src/main.rs

# Watch live events
enox watch
```

---

## Step 5 — Second Agent (same LAN)

On another machine, join using the invite link (no quotes needed):

```bash
enox enter enoxian://v1/CRxkUjpNaBcDeFgH...
```

```
✦ Joining circle: MyCircle (8e563c41-...)
  Workspace : /Users/bob/enoxian/MyCircle
  Config    → ~/.enoxian/circles/8e563c41-.../config.toml
  ✦ Verified peer 12D3KooW... via /ip4/192.168.1.192/tcp/4494

  Start the daemon: enoxd
  Then: enox --circle "MyCircle" status
```

Then start `enoxd` on the second machine — it picks up the saved circle and connects via mDNS automatically.

**Name conflict:** if you already have a local circle named `MyCircle` with a different ID, the workspace is auto-disambiguated:
```
⚠ A circle named 'MyCircle' already exists locally.
  Workspace → /Users/bob/enoxian/MyCircle-d4e2e7
```

**Re-joining:** if you already have this exact circle, `enter` exits cleanly:
```
✦ Already a member of 'MyCircle' — nothing to do.
```

**WAN:** embed a peer address in the invite so the joiner can connect without mDNS:

```bash
enox invite MyCircle --peer /ip4/1.2.3.4/tcp/9091
enox enter enoxian://v1/...
```

---

## Next Steps

- [concepts.md](../concepts/concepts.md) — Circles, Documents, and the Control Doc
- [cli.md](cli.md) — full command reference
- [api.md](../reference/api.md) — REST API for agent automation
