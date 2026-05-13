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
  Generate a new link anytime: enoch invite 8e563c41-...
```

The invite link encodes both the circle ID and the pre-shared key. Share it over a trusted channel (direct message, config file, secrets manager). Do not post it publicly.

---

### `invite`

Generate a new invite link for an existing circle.

```bash
enoch invite <CIRCLE-ID> [--ttl <DURATION>] [--peer <MULTIADDR>]
```

| Flag | Default | Description |
|------|---------|-------------|
| `--ttl` | `7d` | How long the invite is valid |
| `--peer` | — | Embed a peer address so the invitee can connect without mDNS (useful for WAN) |

**Output:**
```
✦ Invite for 'MyCircle' (valid 24h):

  enochian://v1/CRxkUjpN...?expires=2026-05-14T14:00:00Z&name=MyCircle

  Join with: enoch enter "<invite>"
```

With embedded peer (for WAN connections):
```bash
enoch invite 8e563c41-... --ttl 24h --peer /ip4/203.0.113.5/tcp/9091
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
| `--peer` | Override or supplement the peer address (takes priority over any address in the invite) |
| `--rendezvous` | WAN rendezvous server multiaddr |

**Expiry check:** If the invite has expired, `enter` exits immediately with an error:
```
Error: invite expired 2h ago (at 2026-05-13 12:00 UTC)
```

**mDNS (LAN):** On the same network, peers are discovered automatically — no `--peer` flag needed.

**WAN:** Either embed `--peer` in the invite when generating it (`enoch invite --peer ...`), or pass `--peer` directly at join time.

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

---

### `claim`

Claim an open task.

```bash
enoch claim <TASK-ID>
```

The agent ID is read from `ENOCHIAN_AGENT_ID` (default: `"anonymous"`).

---

### `done`

Mark a task as done.

```bash
enoch done <TASK-ID>
```

---

### `bind`

Acquire an advisory file lock.

```bash
enoch bind <PATH>
```

`<PATH>` is relative to the sync directory, forward slashes. Returns an error if another agent holds the lock.

---

### `release`

Release a file lock.

```bash
enoch release <PATH>
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
| `ENOCHIAN_AGENT_ID` | `anonymous` | Agent ID used in `claim` and `bind` |
