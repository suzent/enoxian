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

This produces the unified debug binary at `target/debug/enox`.

### 2. Install the release binary (recommended)

Use the normal release installer once, optionally enabling the login service.
Development updates then replace that same managed binary, so PATH and service
definitions never split across stable and Cargo installations.

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

This fast-forwards the source checkout, builds the release binary, replaces the
currently managed `enox` executable, and restores the previous service mode. It
waits for API health and rolls back automatically if the new daemon fails.

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
# Just rebuild (you restart Enoxian manually)
cargo watch -x "build --bins"

# Rebuild and restart Enoxian automatically
cargo watch -x "build --bins" -s "./target/debug/enox stop || true; sleep 1; ./target/debug/enox start"
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

Check which binary and channel are active:

```bash
enox update --status
```

## Dev vs Stable

| | Dev (`--dev`) | Stable |
|---|---|---|
| Source | Builds from your local clone | Downloads a verified binary from GitHub Releases |
| Rust required | Yes | No |
| Build time | ~5s incremental | Instant |
| Use when | Actively developing | End users just want the latest |

Rerun the release installer at any time to switch the same managed path back to
the stable channel.

---

## Logs

```bash
# Run daemon with full logs
RUST_LOG=info enox daemon run           # Mac / Linux
$env:RUST_LOG = "info"; enox daemon run # Windows PowerShell

# More verbose (includes debug output from enoxian crate)
RUST_LOG=debug enox daemon run

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

**Enoxian won't start after update**
The updater restores the previous binary automatically when its 20-second API
health check fails. Inspect `enox update --status` and the service logs before
retrying.

**Circles not discovered across machines**
Both machines must be on the same LAN for mDNS to work. For WAN, use a Circle
invite with its embedded rendezvous/relay address.

**`cargo install` is slow**
First install compiles everything from scratch (~2–3 minutes). After that, incremental rebuilds only recompile changed files (~5–15 seconds).
