# CLI Reference — `enoch`

The `enoch` binary is the agent-facing CLI. It is stateless — every invocation makes one or more HTTP calls to the daemon and exits.

```
Usage: enoch [OPTIONS] <COMMAND>

Options:
  --json    Output raw JSON instead of human-readable text
  -h, --help
```

The target daemon is configured via the `ENOCHIAN_API` environment variable (default: `http://127.0.0.1:9090/api`).

---

## Commands

### `init`

Create a new Circle.

```bash
enoch init --name <NAME>
```

Generates a fresh Ed25519 keypair and a random 256-bit PSK. Saves config to `~/.enochian/circles/<id>/config.toml`.

**Output:**
```
✦ Circle cast: MyCircle
  circle-id : 8e563c41-f0ec-4225-9764-064f1fb04341
  peer-id   : 12D3KooW...
  secret    : d2d89de6...
```

---

### `enter`

Join an existing Circle via P2P dial.

```bash
enoch enter <CIRCLE-ID> --secret <HEX> [--peer <MULTIADDR>] [--rendezvous <MULTIADDR>]
```

| Flag | Description |
|------|-------------|
| `--secret` | Pre-shared key (hex), shared by the circle creator |
| `--peer` | Directly dial a peer multiaddr (e.g. `/ip4/1.2.3.4/tcp/9091`) |
| `--rendezvous` | WAN rendezvous server multiaddr |

On a LAN, mDNS discovers peers automatically — no `--peer` is needed.

---

### `status`

Show Circle overview.

```bash
enoch status
```

```
◆ Circle:  MyCircle
  ID:      8e563c41-...
  SyncDir: ~/.enochian/circles/.../files
  Docs:    3
```

---

### `who`

Show registered agent presence.

```bash
enoch who
```

```
  agent-alpha   active    2026-05-13 14:32:01
  agent-beta    idle      2026-05-13 14:29:44
```

---

### `tasks`

List tasks, optionally filtered by status.

```bash
enoch tasks [--status open|claimed|done]
```

```
  [open]    4873c16e  Write integration tests
  [claimed] a2853491  Refactor network layer  (→ agent-beta)
  [done]    f1e2d3c4  Update README
```

Tasks are sorted by `created_at` ascending.

---

### `claim`

Claim an open task.

```bash
enoch claim <TASK-ID>
```

The agent ID is read from the `ENOCHIAN_AGENT_ID` environment variable (default: `"anonymous"`).

```
✦ claimed: 4873c16e-15c8-4ddb-9598-c0ad85395862
```

---

### `done`

Mark a task as done.

```bash
enoch done <TASK-ID>
```

```
✦ done: 4873c16e-15c8-4ddb-9598-c0ad85395862
```

---

### `bind`

Acquire an advisory file lock.

```bash
enoch bind <PATH>
```

`<PATH>` is relative to the sync directory, using forward slashes. Returns an error if another agent holds the lock.

```
✦ bound: src/main.rs
```

On conflict:
```
✗ conflict: src/main.rs is held by agent-beta
```

---

### `release`

Release a file lock.

```bash
enoch release <PATH>
```

```
✦ released: src/main.rs
```

---

### `watch`

Stream live Circle events. Blocks until Ctrl+C.

```bash
enoch watch
```

```
◆ Watching circle events (Ctrl+C to stop)...
  [task_created]  {"type":"task_created","task_id":"..."}
  [lock_acquired] {"type":"lock_acquired","path":"src/main.rs","agent_id":"agent-alpha"}
  [file_updated]  {"type":"file_updated","path":"notes.txt"}
```

---

## Environment Variables

| Variable | Default | Purpose |
|----------|---------|---------|
| `ENOCHIAN_API` | `http://127.0.0.1:9090/api` | Daemon base URL |
| `ENOCHIAN_AGENT_ID` | `anonymous` | Agent ID sent in `claim` / `bind` requests |
