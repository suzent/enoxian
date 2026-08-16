# Getting Started

## Install (prebuilt binaries)

The quickest path — downloads the latest release for your platform and installs
`enox` and `enoxd`.

**Linux / macOS:**

```sh
curl -fsSL https://github.com/suzent/enoxian/releases/latest/download/install.sh | sh
```

**Windows:**

```powershell
irm https://github.com/suzent/enoxian/releases/latest/download/install.ps1 | iex
```

Every release includes `SHA256SUMS`, which the installers verify before
installation.

**Homebrew** (once the tap is published):

```sh
brew install suzent/tap/enoxian
```

Pin a version with `ENOXIAN_VERSION=v0.2.1` (or `$env:ENOXIAN_VERSION`), or set
`ENOXIAN_BIN_DIR` to change the install directory. To build from source instead,
see below.

## Prerequisites (build from source)

- Rust 1.88 or newer
- Cargo
- Node.js (only for building the frontend in release mode)

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

### Install to PATH

To use `enox` and `enoxd` as plain commands without a path prefix, install both
binaries from source:

```bash
cargo install --path . --bins
```

After that, rebuild and reinstall in one step:

```bash
enox update --dev --src .
```

On subsequent runs the `--src` path is remembered, so `enox update --dev` is
enough. On Windows, the running `enox.exe` is replaced via a deferred PowerShell
script after the process exits; `enoxd` is restarted automatically.

---

## Step 1 — Create a Circle

A Circle is the shared workspace. Run this once on any machine:

```bash
./target/debug/enox init --name MyCircle
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

The workspace directory (`~/enoxian/MyCircle`) is created automatically. This is
where shared files live. The invite link encodes the circle ID, PSK, expiry, and
optional connectivity hints; share it over a trusted channel.

To use a different workspace directory:

```bash
enox init --name "MyCircle" --dir ~/projects/myapp
```

---

## Step 2 — Start the Daemon

`enoxd` loads all enabled circles from `~/.enoxian/circles/` and serves them over
one local HTTP/WebSocket API port:

```bash
# bash / MSYS2
RUST_LOG=info ./target/debug/enoxd

# PowerShell
$env:RUST_LOG = "info"
.\target\debug\enoxd.exe
```

You can also start it in the background:

```bash
enox start
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
INFO  HTTP/WS listening on 127.0.0.1:36521
INFO  [8e563c41-...] P2P listening on /ip4/192.168.1.x/tcp/<random>
```

All circles share one HTTP port. Each enabled circle gets its own P2P swarm on a
random port.

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

# Create a task
enox task-create "Write integration tests" --description "Cover lock arbitration"

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

# Open the local web UI
enox open
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

Then start `enoxd` on the second machine. It picks up the saved circle and
connects over mDNS on the same LAN.

**Name conflict:** if you already have a local circle named `MyCircle` with a different ID, the workspace is auto-disambiguated:
```
⚠ A circle named 'MyCircle' already exists locally.
  Workspace → /Users/bob/enoxian/MyCircle-d4e2e7
```

**Re-joining:** if you already have this exact circle, `enter` exits cleanly:
```
✦ Already a member of 'MyCircle' — nothing to do.
```

**WAN:** current invites try to embed relay/rendezvous addresses automatically
when available. You can also pass an explicit peer, relay, or rendezvous address:

```bash
enox invite MyCircle --peer /ip4/1.2.3.4/tcp/9091
enox invite MyCircle --rendezvous enox.yourdomain.com
enox enter enoxian://v1/...
```

See [invite.md](invite.md) and
[rendezvous-setup.md](../reference/rendezvous-setup.md) for WAN setup.

---

## Next Steps

- [concepts.md](../concepts/concepts.md) — Circles, Documents, and the Control Doc
- [cli.md](cli.md) — full command reference
- [api.md](../reference/api.md) — REST API for agent automation
