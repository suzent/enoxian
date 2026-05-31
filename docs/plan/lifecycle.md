# Circle Lifecycle Management — Design & Implementation Plan

## Problem

All circles load at daemon startup and run until the daemon is killed. There is no way to:
- Temporarily pause a circle without deleting it
- Leave a circle permanently
- Toggle individual circles while `enoxd` is running

## Operations

### Disable (temporary pause)

Stops the circle's P2P swarm and file watcher. Config and workspace files are preserved. The circle can be re-enabled at any time.

Use case: a project you're not actively working on, or a circle you want to stop syncing without leaving.

```bash
enox disable MyCircle
# ◆ MyCircle paused. Run `enox enable MyCircle` to resume.

enox enable MyCircle
# ◆ MyCircle resumed.
```

### Leave (permanent removal)

Removes the circle config from this machine. The workspace directory is left intact by default (files are yours). Other peers are unaffected — they keep the circle.

```bash
enox leave MyCircle
# Are you sure you want to leave 'MyCircle'? [y/N] y
# ◆ Left MyCircle. Workspace kept at ~/enoxian/MyCircle
# To also remove the workspace: rm -rf ~/enoxian/MyCircle
```

This is irreversible locally. To rejoin, you need a new invite from another member.

### Runtime start/stop (no daemon restart)

`enox disable` and `enox enable` call the daemon API to take effect immediately without restarting `enoxd`. The `disabled` flag in config persists the state across restarts.

---

## Config changes

Add `disabled` field to `CircleConfig`:

```toml
circle_id         = "8e563c41-..."
circle_name       = "MyCircle"
psk_hex           = "d2d89de6..."
keypair_proto_hex = "0802..."
workspace_dir     = "/Users/suzy/enoxian/MyCircle"
disabled          = true          # optional, default false
```

`enoxd` skips circles where `disabled = true` at startup.

---

## CLI changes

### `enox disable <CIRCLE>`
1. Resolve circle by name/prefix
2. Set `disabled = true` in `~/.enoxian/circles/<id>/config.toml`
3. Call `POST /circles/<id>/stop` if daemon is running (graceful — not an error if daemon is down)

### `enox enable <CIRCLE>`
1. Resolve circle (including disabled ones — load_all must return them)
2. Clear `disabled` flag in config
3. Call `POST /circles/<id>/start` if daemon is running

### `enox leave <CIRCLE>`
1. Resolve circle
2. Print confirmation prompt (skip with `--yes`)
3. Call `POST /circles/<id>/stop` if daemon is running
4. Delete `~/.enoxian/circles/<id>/` directory
5. Print workspace path — do NOT delete workspace (user's files)

### `enox circles` output

```
  MyCircle    — 8e563c41-...   ~/enoxian/MyCircle
  WorkProject — 2a3b4c5d-...  ~/enoxian/WorkProject  [paused]
```

---

## API changes

### `POST /circles/<id>/stop`

Stops the circle's P2P swarm task and file watcher. Removes the circle from `DaemonState`.

Response: `{"status": "stopped", "circle_id": "..."}`
Returns 404 if the circle is not currently active.

### `POST /circles/<id>/start`

Re-loads the circle config from disk, spawns a new P2P swarm + file watcher, inserts into `DaemonState`.

Response: `{"status": "started", "circle_id": "..."}`
Returns 409 if the circle is already running.

---

## Implementation tasks

### 1. `src/config.rs`
- [ ] Add `disabled: bool` field with `#[serde(default)]`
- [ ] `load_all()` returns all circles including disabled ones
- [ ] Add `load_all_active()` that filters out disabled ones — used by `enoxd` startup

### 2. `src/commands/serve.rs`
- [ ] Use `load_all_active()` instead of `load_all()` at startup

### 3. `src/daemon.rs`
- [ ] Add `remove(&self, circle_id: &str)` method to `DaemonState`

### 4. `src/api/mod.rs`
- [ ] Add `POST /circles/{circle_id}/stop` route
- [ ] Add `POST /circles/{circle_id}/start` route

### 5. `src/api/lifecycle.rs` (new file)
- [ ] `stop_circle` handler — abort swarm task + watcher, remove from DaemonState, return 200
- [ ] `start_circle` handler — load config, spawn swarm + watcher, insert into DaemonState, return 200

### 6. `src/cli.rs`
- [ ] Add `Disable { circle: String }` to `AgentCommands`
- [ ] Add `Enable { circle: String }` to `AgentCommands`
- [ ] Add `Leave { circle: String, #[arg(long)] yes: bool }` to `AgentCommands`

### 7. `src/commands/disable.rs` (new)
- [ ] Set `disabled = true`, save config, call `POST /circles/<id>/stop`

### 8. `src/commands/enable.rs` (new)
- [ ] Clear `disabled`, save config, call `POST /circles/<id>/start`

### 9. `src/commands/leave.rs` (new)
- [ ] Confirmation prompt, call stop API, delete config dir, print workspace path

### 10. `src/commands/circles.rs`
- [ ] Show `[paused]` marker for disabled circles

### 11. `src/bin/enox.rs`
- [ ] Wire up Disable, Enable, Leave commands

### 12. Docs
- [ ] Update `cli.md` — disable, enable, leave commands
- [ ] Update `daemon.md` — disabled field in config.toml

---

## What does NOT change

- Workspace files are never touched by `disable` or `leave` — they belong to the user
- Other peers are unaffected by any local lifecycle operation
- The CRDT state is preserved in memory until `stop` — no data loss on disable
