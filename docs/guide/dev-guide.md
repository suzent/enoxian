# Developer Guide

This guide is for people actively developing enoxian across multiple machines.

---

## Initial Setup

### 1. Clone and build

```bash
# On each machine, clone wherever you like
git clone <repo> ~/enoxian      # Mac / Linux
git clone <repo> D:\workspace\enoxian   # Windows
```

```bash
cd ~/enoxian
cargo build --bins
```

This produces debug binaries at `target/debug/enoxd` and `target/debug/enox`.

### 2. Install to PATH (recommended)

`cargo install` puts `enox` and `enoxd` into `~/.cargo/bin/`, which is already in your PATH after Rust is installed. After this you can just type `enox` from anywhere.

```bash
cargo install --path .
```

Run this once per machine. After that, use `enox update --dev` to keep them current.

### 3. Register the source directory

Tell `enox` where the source lives on this machine. This is saved to `~/.enoxian/config.toml` and never needs to be set again.

```bash
# Mac / Linux
enox update --dev --src /path/to/enoxian

# Windows (PowerShell)
enox update --dev --src /path/to/enoxian
```

The `--src` path is just the folder you cloned the repo into — wherever `Cargo.toml` lives.

---

## Day-to-Day Workflow

### Updating a single machine

```bash
enox update --dev
```

This runs `git pull`, rebuilds with `cargo install`, and restarts `enoxd` automatically.

To rebuild without pulling (e.g. you made local edits):

```bash
enox update --dev --no-pull
```

### Updating via script (Mac / Linux)

If you prefer a shell script over the CLI:

```bash
./scripts/dev-sync.sh
```

This does the same thing: pull → build → restart daemon.

### Auto-rebuild during active development

While editing code, use `cargo watch` to rebuild on every save:

```bash
# Just rebuild (you restart enoxd manually)
cargo watch -x "build --bins"

# Rebuild and restart enoxd automatically
cargo watch -x "build --bins" -s "pkill -f enoxd; sleep 1; ./target/debug/enoxd &"
```

---

## Multi-machine Workflow

A typical setup: you edit code on one machine, test sync between two machines.

**On the editing machine** — push changes when ready:

```bash
git add -p && git commit -m "..." && git push
```

**On the other machine** — pull and update in one command:

```bash
enox update --dev
```

Both machines always run the same version.

---

## Dev vs Stable

| | Dev (`--dev`) | Stable (future) |
|---|---|---|
| Source | Builds from your local clone | Downloads pre-built binary from GitHub Releases |
| Rust required | Yes | No |
| Build time | ~5s incremental | Instant |
| Use when | Actively developing | End users just want the latest |

Stable binary downloads are not yet available — they are planned in M12 (Packaging & Distribution). Until then all users build from source.

---

## Logs

```bash
# Run daemon with full logs
RUST_LOG=info enoxd           # Mac / Linux
$env:RUST_LOG = "info"; enoxd # Windows PowerShell

# More verbose (includes debug output from enoxian crate)
RUST_LOG=debug enoxd

# dev-sync.sh logs to:
~/.enoxian/daemon.log
```

---

## Common Issues

**`enox update --dev` fails with "no source path configured"**
Run once with `--src` to save the path:
```bash
enox update --dev --src /path/to/enoxian
```

**`enoxd` won't start after update**
The old process may still be running. Kill it manually:
```bash
pkill -f enoxd          # Mac / Linux
taskkill /F /IM enoxd.exe   # Windows
```

**Circles not discovered across machines**
Both machines must be on the same LAN for mDNS to work. For WAN, use an anchor node (M11).

**`cargo install` is slow**
First install compiles everything from scratch (~2–3 minutes). After that, incremental rebuilds only recompile changed files (~5–15 seconds).
