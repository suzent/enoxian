# Workspace Folders — Design & Implementation Plan

## Problem

The current sync directory is `~/.enochian/circles/<uuid>/files/`. This is:
- **Hidden** — not visible in Finder/Explorer without showing hidden files
- **UUID-named** — not human-readable
- **Inconvenient** — you have to copy files into it rather than work in place

## Design

Each circle gets a named, visible workspace directory.

**Default location:** `~/enochian/<circle-name>/`

```
~/enochian/
  MyCircle/          ← workspace for circle "MyCircle"
    notes.md
    tasks/
    src/
  WorkProject/       ← workspace for circle "WorkProject"
    README.md
    ...
```

**Custom location** (set at init time or any time):

```bash
enoch init --name WorkProject --dir ~/projects/myapp
```

The workspace path is stored in `config.toml` alongside the PSK and keypair. The hidden `~/.enochian/` directory remains for config only — workspace files live somewhere the user can actually see them.

---

## Config changes

Add `workspace_dir` to `CircleConfig`:

```toml
# ~/.enochian/circles/<id>/config.toml
circle_id         = "8e563c41-..."
circle_name       = "MyCircle"
psk_hex           = "d2d89de6..."
keypair_proto_hex = "0802..."
workspace_dir     = "/Users/suzy/enochian/MyCircle"
```

Migration: circles without `workspace_dir` fall back to `~/.enochian/circles/<id>/files/` (current behaviour).

---

## Name conflict handling

Circle names are not globally unique — two people on different machines can independently create circles with the same name. Conflicts must be handled locally.

### `enoch init` — error on duplicate name

You are choosing the name, so a duplicate is a mistake. Reject it:

```
Error: a circle named 'MyCircle' already exists.
       Run `enoch circles` to list existing circles, or choose a different name.
```

Implementation: check `config::load_all()` before creating the circle. If any existing circle has the same `circle_name`, bail.

### `enoch enter` — auto-resolve silently

You do not control the name — the circle founder chose it. Two auto-handled cases:

**Case 1: Same UUID (re-joining a circle you already have)**
Detect by UUID match, skip saving, exit with a message:
```
✦ Already a member of MyCircle — nothing to do.
  Run: enochd
```

**Case 2: Same name, different UUID (two unrelated circles with the same name)**
Keep the circle name in config as-is. Disambiguate only the workspace folder by appending a short UUID prefix:
```
⚠ A circle named 'MyCircle' already exists locally.
  Workspace → ~/enochian/MyCircle-d4e2e7
```

The user can move/rename the folder later. The `circle_name` field in config always stays as received from the invite.

---

## CLI changes

### `enoch init`

```bash
enoch init --name MyCircle                    # workspace → ~/enochian/MyCircle
enoch init --name MyCircle --dir ~/projects   # workspace → ~/projects
```

New `--dir` flag. If omitted, defaults to `~/enochian/<circle-name>/`. Directory is created if it does not exist.

### `enoch enter`

```bash
enoch enter enochian://v1/...                  # workspace → ~/enochian/<circle-name>
enoch enter enochian://v1/... --dir ~/projects # custom location
```

New `--dir` flag. If omitted, defaults to `~/enochian/<circle-name>/`, with auto-disambiguation if that name is taken by a different circle.

### `enoch status`

Output gains a `Workspace` line:

```
◆ Circle:    MyCircle
  ID:        8e563c41-...
  Workspace: ~/enochian/MyCircle
  Docs:      3
```

---

## Implementation tasks

### 1. `src/config.rs`
- [x] Add `workspace_dir: String` field to `CircleConfig`
- [x] Add `#[serde(default)]` fallback so existing configs without the field still load
- [x] Add `default_workspace_dir(circle_name) -> PathBuf` helper → `~/enochian/<name>`
- [x] Add `resolve_workspace_dir(...)` — handles both conflict cases, returns `None` for re-join

### 2. `src/cli.rs`
- [x] Add `--dir <PATH>` to `InitArgs`
- [x] Add `--dir <PATH>` to `EnterArgs`

### 3. `src/commands/init.rs`
- [x] Check `load_all()` for duplicate `circle_name` — bail if found
- [x] Resolve workspace dir: `--dir` flag or `default_workspace_dir(&args.name)`
- [x] `tokio::fs::create_dir_all` the workspace
- [x] Store resolved path in `CircleConfig.workspace_dir`
- [x] Print workspace path in output

### 4. `src/commands/enter.rs`
- [x] Check `load_all()` for same UUID — if found, print "already a member" and exit
- [x] Check `load_all()` for same name, different UUID — use disambiguated workspace dir
- [x] Resolve workspace dir: `--dir` flag or disambiguated default
- [x] `tokio::fs::create_dir_all` the workspace
- [x] Warn user if workspace was disambiguated
- [x] Store resolved path in saved `CircleConfig.workspace_dir`

### 5. `src/commands/serve.rs`
- [x] Read `config.workspace_dir` instead of hardcoding `circle_dir/files`
- [x] Pass workspace path to `AppState::new` and `spawn_watcher`

### 6. `src/api/status.rs`
- [x] Include `workspace` in status response

### 7. Docs
- [ ] Update `getting-started.md` — new init output, workspace path
- [ ] Update `cli.md` — `--dir` flag for init and enter
- [ ] Update `daemon.md` — `workspace_dir` in config.toml example

---

## What does NOT change

- `~/.enochian/circles/<id>/config.toml` — config always stays here
- Circle resolution by name/prefix — unchanged
- REST API routes — unchanged
- CRDT / file watcher internals — unchanged, just pointed at a different directory
