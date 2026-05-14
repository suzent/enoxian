# Developer Guide

This guide is for people actively developing ENOCHIAN across multiple machines.

---

## Initial Setup

### 1. Clone and build

```bash
# On each machine, clone wherever you like
git clone <repo> ~/enochian      # Mac / Linux
git clone <repo> D:\workspace\enochian   # Windows
```

```bash
cd ~/enochian
cargo build --bins
```

This produces debug binaries at `target/debug/enochd` and `target/debug/enoch`.

### 2. Install to PATH (recommended)

`cargo install` puts `enoch` and `enochd` into `~/.cargo/bin/`, which is already in your PATH after Rust is installed. After this you can just type `enoch` from anywhere.

```bash
cargo install --path .
```

Run this once per machine. After that, use `enoch update --dev` to keep them current.

### 3. Register the source directory

Tell `enoch` where the source lives on this machine. This is saved to `~/.enochian/config.toml` and never needs to be set again.

```bash
# Mac / Linux
enoch update --dev --src /path/to/enochian

# Windows (PowerShell)
enoch update --dev --src /path/to/enochian
```

The `--src` path is just the folder you cloned the repo into — wherever `Cargo.toml` lives.

---

## Day-to-Day Workflow

### Updating a single machine

```bash
enoch update --dev
```

This runs `git pull`, rebuilds with `cargo install`, and restarts `enochd` automatically.

To rebuild without pulling (e.g. you made local edits):

```bash
enoch update --dev --no-pull
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
# Just rebuild (you restart enochd manually)
cargo watch -x "build --bins"

# Rebuild and restart enochd automatically
cargo watch -x "build --bins" -s "pkill -f enochd; sleep 1; ./target/debug/enochd &"
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
enoch update --dev
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
RUST_LOG=info enochd           # Mac / Linux
$env:RUST_LOG = "info"; enochd # Windows PowerShell

# More verbose (includes debug output from enochian crate)
RUST_LOG=debug enochd

# dev-sync.sh logs to:
~/.enochian/daemon.log
```

---

## Common Issues

**`enoch update --dev` fails with "no source path configured"**
Run once with `--src` to save the path:
```bash
enoch update --dev --src /path/to/enochian
```

**`enochd` won't start after update**
The old process may still be running. Kill it manually:
```bash
pkill -f enochd          # Mac / Linux
taskkill /F /IM enochd.exe   # Windows
```

**Circles not discovered across machines**
Both machines must be on the same LAN for mDNS to work. For WAN, use an anchor node (M11).

**`cargo install` is slow**
First install compiles everything from scratch (~2–3 minutes). After that, incremental rebuilds only recompile changed files (~5–15 seconds).
