# CLI Reference — `enoch`

The `enoch` binary is the agent-facing CLI. It is stateless — every invocation makes one or more HTTP calls to the daemon and exits.

```
Usage: enoch [OPTIONS] <COMMAND>

Options:
  --json              Output raw JSON instead of human-readable text
  --circle <NAME>     Target circle by name, name prefix, or UUID prefix
                      (overrides ENOCHIAN_CIRCLE env var)
  -h, --help
```

Circle resolution order: exact name → case-insensitive name prefix → UUID prefix → error if ambiguous. If only one circle exists, it is selected automatically and `--circle` is optional.

The target daemon URL is configured via `ENOCHIAN_API` (default: `http://127.0.0.1:9090`).

---

## Commands

### `init`

Create a new Circle and print a shareable invite link.

```bash
enoch init --name <NAME> [--ttl <DURATION>]
```

| Flag | Default | Description |
|------|---------|-------------|
| `--name` | required | Human-readable circle name |
| `--ttl` | `7d` | Validity of the generated invite link (`7d`, `24h`, etc.) |

**Output:**
```
✦ Circle cast: MyCircle
  circle-id : 8e563c41-f0ec-4225-9764-064f1fb04341
  peer-id   : 12D3KooW...

  invite    : enochian://v1/CRxkUjpN...?expires=2026-05-20T14:00:00Z&name=MyCircle

  Share the invite link to let peers join (valid for 7d).
  Generate a new link anytime: enoch invite MyCircle
```

---

### `circles`

List known circles. If the daemon is running, shows active circles. Falls back to local configs if the daemon is not reachable.

```bash
enoch circles
```

```
  MyCircle    — 8e563c41-f0ec-4225-9764-064f1fb04341
  WorkProject — 2a3b4c5d-...
```

---

### `invite`

Generate a new invite link for an existing circle.

```bash
enoch invite <CIRCLE> [--ttl <DURATION>] [--peer <MULTIADDR>]
```

`<CIRCLE>` is resolved by name, name prefix, or UUID prefix.

| Flag | Default | Description |
|------|---------|-------------|
| `--ttl` | `7d` | How long the invite is valid |
| `--peer` | — | Embed a peer address for WAN connections |

**Output:**
```
✦ Invite for 'MyCircle' (valid 24h):

  enochian://v1/CRxkUjpN...?expires=2026-05-14T14:00:00Z&name=MyCircle

  Join with: enoch enter "<invite>"
```

---

### `enter`

Join a Circle using an invite link or a raw circle ID + secret.

```bash
# Recommended — single invite link
enoch enter "enochian://v1/CRxkUjpN...?expires=...&name=MyCircle"

# Legacy — explicit flags
enoch enter <CIRCLE-ID> --secret <HEX>
```

| Flag | Description |
|------|-------------|
| `--secret` | Pre-shared key hex — required when target is a raw Circle ID |
| `--peer` | Override or supplement the peer address |
| `--rendezvous` | WAN rendezvous server multiaddr |

**Expiry check:** If the invite has expired, `enter` exits immediately with an error.

---

### `status`

Show circle overview.

```bash
enoch [--circle <NAME>] status
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
enoch [--circle <NAME>] who
```

---

### `tasks`

List tasks, optionally filtered by status.

```bash
enoch [--circle <NAME>] tasks [--status open|claimed|done]
```

---

### `claim`

Claim an open task.

```bash
enoch [--circle <NAME>] claim <TASK-ID>
```

---

### `done`

Mark a task as done.

```bash
enoch [--circle <NAME>] done <TASK-ID>
```

---

### `bind`

Acquire an advisory file lock.

```bash
enoch [--circle <NAME>] bind <PATH>
```

`<PATH>` is relative to the sync directory, forward slashes.

---

### `release`

Release a file lock.

```bash
enoch [--circle <NAME>] release <PATH>
```

---

### `watch`

Stream live circle events. Blocks until Ctrl+C.

```bash
enoch [--circle <NAME>] watch
```

---

## Environment Variables

| Variable | Default | Purpose |
|----------|---------|---------|
| `ENOCHIAN_API` | `http://127.0.0.1:9090` | Daemon base URL |
| `ENOCHIAN_CIRCLE` | — | Default circle (name, prefix, or UUID prefix) |
| `ENOCHIAN_AGENT_ID` | `anonymous` | Agent ID used in `claim` and `bind` |
