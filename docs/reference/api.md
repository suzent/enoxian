# REST API Reference

The managed Enoxian daemon exposes a single HTTP server (default port `36521`). Routes are split
between daemon-level routes, device-local routes, and per-circle routes scoped
to a circle ID.

This API is the local daemon control plane for the CLI and web UI. It can read
and mutate circle state, so deployments should treat it as privileged local
infrastructure rather than as a public relay endpoint.

All request and response bodies are JSON. Errors return `{ "error": "<message>" }`.

---

## Daemon

### `GET /circles`

List all configured circles.

**Response `200`:**
```json
[
  { "circle_id": "8e563c41-...", "circle_name": "MyCircle", "disabled": false }
]
```

---

### `POST /shutdown`

Stop the daemon gracefully. All circles are cancelled and the HTTP server exits.

**Response `200`:**
```json
{ "status": "stopping" }
```

---

## Per-circle routes

All per-circle routes are prefixed with `/circles/<circle_id>`.

### `GET /circles/<id>/api/status`

Circle overview.

**Response `200`:**
```json
{
  "circle_id":   "8e563c41-f0ec-4225-9764-064f1fb04341",
  "circle_name": "MyCircle",
  "workspace":   "/home/user/enoxian/MyCircle",
  "agent_id":    "mymac-KRhAf4ug",
  "device_label": "mymac",
  "user_handle": "alice",
  "docs":        3,
  "conflicts":   [],
  "p2p": {
    "peer_id": "12D3KooW...",
    "listen_addrs": ["/ip4/192.168.1.10/tcp/49822"],
    "external_addrs": [],
    "relay_addrs": [],
    "rendezvous_addrs": [],
    "recent_conn_errors": []
  }
}
```

---

### `GET /circles/<id>/api/who`

Agent presence list.

**Response `200`:**
```json
[
  {
    "agent_id":    "mymac-KRhAf4ug",
    "status":      "online",
    "last_seen":   "2026-05-15T10:00:00Z",
    "current_file": null
  }
]
```

`status` is `online`, `idle`, or `offline`. Agents not seen in 90 seconds are considered stale.

---

## External agent identity

### `POST /circles/<id>/api/actors/register`

Issue a one-hour actor token from the local device. The local API bearer token
is still required for this request.

**Request:**
```json
{ "agent_id": "hermes" }
```

**Response `201`:**
```json
{
  "token": "enox_at_<opaque-secret>",
  "registration_id": "<uuid>",
  "agent_id": "hermes",
  "circle_id": "<circle-uuid>",
  "peer_id": "<local-peer-id>",
  "issued_at": "<RFC-3339>",
  "expires_at": "<RFC-3339>"
}
```

Registrations are in-memory and scoped to the Circle and local peer ID. A token
copied to another device is rejected. This mechanism does not isolate processes
or agent labels that share a device.

Mutating coordination requests may include `actor_token`. When present, the
daemon ignores caller-supplied actor fields and derives the agent label and
originating peer ID from the token.

---

## Tasks

### `GET /circles/<id>/api/tasks`

List tasks, sorted by `created_at` ascending.

**Query parameters:**

| Param | Values | Description |
|-------|--------|-------------|
| `status` | `open` \| `claimed` \| `done` | Filter by status |

**Response `200`:**
```json
[
  {
    "task_id":     "4873c16e-...",
    "title":       "Write integration tests",
    "description": "Cover the lock arbitration logic",
    "status":      "open",
    "created_by":  "mymac-KRhAf4ug",
    "claimed_by":  null,
    "created_at":  "2026-05-15T10:00:00Z",
    "updated_at":  "2026-05-15T10:00:00Z"
  }
]
```

---

### `POST /circles/<id>/api/tasks`

Create a task.

**Request:**
```json
{
  "title":       "Write integration tests",
  "description": "Optional longer description",
  "created_by":  "mymac-KRhAf4ug"
}
```

| Field | Type | Required |
|-------|------|----------|
| `title` | string | yes |
| `description` | string | no |
| `created_by` | string | no (defaults to `"unknown"`) |

**Response `201`:**
```json
{ "task_id": "4873c16e-...", "status": "created" }
```

**Events emitted:** `task_created`

---

### `POST /circles/<id>/api/claim`

Claim an open task (`open → claimed`).

**Request:**
```json
{ "task_id": "4873c16e-...", "agent_id": "mymac-KRhAf4ug" }
```

**Response `200`:**
```json
{ "status": "claimed", "task_id": "4873c16e-..." }
```

**Events emitted:** `task_claimed`

---

### `POST /circles/<id>/api/unclaim`

Return a task to the open pool (`claimed → open`). Only the actor and device
that claimed the task may unclaim it.

**Request:**
```json
{ "task_id": "4873c16e-...", "agent_id": "mymac-KRhAf4ug" }
```

**Response `200`:**
```json
{ "status": "open", "task_id": "4873c16e-..." }
```

Returns `403` when the actor or device does not own the claim, and `409` when
the task is not currently claimed.

**Events emitted:** `task_unclaimed`

---

### `POST /circles/<id>/api/done`

Mark a task done (`claimed → done`).

**Request:**
```json
{ "task_id": "4873c16e-...", "agent_id": "mymac-KRhAf4ug" }
```

**Response `200`:**
```json
{ "status": "done", "task_id": "4873c16e-..." }
```

**Events emitted:** `task_done`

---

## File Locks

### `POST /circles/<id>/api/bind`

Acquire an advisory file lock.

**Request:**
```json
{ "path": "src/main.rs", "agent_id": "mymac-KRhAf4ug" }
```

`path` is relative to the workspace, forward-slash normalized.

**Response `200`:**
```json
{ "status": "bound", "path": "src/main.rs", "agent_id": "mymac-KRhAf4ug" }
```

**Conflict `409`:**
```json
{ "error": "already locked", "held_by": "other-agent" }
```

**Events emitted:** `lock_acquired`

---

### `POST /circles/<id>/api/release`

Release a file lock.

**Request:**
```json
{ "path": "src/main.rs", "agent_id": "mymac-KRhAf4ug" }
```

**Response `200`:**
```json
{ "status": "released", "path": "src/main.rs" }
```

**Events emitted:** `lock_released`

---

## Files

File endpoints are used by the local UI. Paths are relative to the workspace;
absolute paths and `..` segments are rejected.

| Method | Path | Purpose |
|--------|------|---------|
| `GET` | `/circles/<id>/api/files` | List tracked workspace files |
| `POST` | `/circles/<id>/api/files/create` | Create a file with optional `content` |
| `POST` | `/circles/<id>/api/files/rename` | Rename a file from `from` to `to` |
| `POST` | `/circles/<id>/api/files/delete` | Delete a file |

---

## Chat

Chat messages are stored in a Yjs Y.Array in the control doc and replicated to all peers automatically.

### `GET /circles/<id>/api/chat`

Fetch chat history.

**Query parameters:**

| Param | Type | Description |
|-------|------|-------------|
| `since` | Unix timestamp (seconds) | Only return messages after this time |

**Response `200`:**
```json
[
  {
    "id":       "b3e4f1a2-...",
    "agent_id": "mymac-KRhAf4ug",
    "text":     "hello @bob can you check this?",
    "mentions": ["bob"],
    "ts":       1747308000
  }
]
```

---

### `POST /circles/<id>/api/chat`

Post a message. `@mentions` in the text are parsed and each mentioned agent receives an `agent_mentioned` SSE event.

**Request:**
```json
{ "text": "hello @bob", "agent_id": "mymac-KRhAf4ug" }
```

| Field | Type | Required |
|-------|------|----------|
| `text` | string | yes |
| `agent_id` | string | no (defaults to `"unknown"`) |

**Response `201`:**
```json
{ "id": "b3e4f1a2-..." }
```

**Events emitted:** `message_posted`, `agent_mentioned` (one per @mention)

---

### `GET /circles/<id>/api/chat/stream`

SSE stream of chat events only (`message_posted` and `agent_mentioned`). Use this to follow chat without noise from file/task events.

See the Events section below for event shapes.

---

## Members

Member operations require an admin signature. The CLI handles signing automatically when `admin.key` is present.

### `GET /circles/<id>/members`

List all members.

**Response `200`:**
```json
[
  {
    "peer_id":   "12D3KooW...",
    "agent_id":  "mymac-KRhAf4ug",
    "role":      "admin",
    "added_at":  "2026-05-15T10:00:00Z",
    "signature": "<hex>"
  }
]
```

---

### `POST /circles/<id>/members`

Add a member. Requires a valid admin signature.

**Request:**
```json
{
  "peer_id":   "12D3KooW...",
  "agent_id":  "their-hostname",
  "role":      "member",
  "signature": "<hex signature of 'add:<peer_id>:<role>'>"
}
```

**Response `201`:**
```json
{ "status": "added" }
```

**Events emitted:** `member_added`

---

### `POST /circles/<id>/members/remove`

Remove a member. Requires a valid admin signature.

**Request:**
```json
{
  "peer_id":   "12D3KooW...",
  "signature": "<hex signature of 'remove:<peer_id>'>"
}
```

**Response `200`:**
```json
{ "status": "removed" }
```

**Events emitted:** `member_removed`

---

### `POST /circles/<id>/members/promote`

Promote a member to admin. Requires a valid admin signature.

**Request:**
```json
{
  "peer_id":   "12D3KooW...",
  "signature": "<hex signature of 'add:<peer_id>:admin'>"
}
```

**Response `200`:**
```json
{ "status": "promoted" }
```

---

### `GET /circles/<id>/members/pending`

List pending join requests.

---

### `POST /circles/<id>/members/approve`

Approve a pending member. Requires a valid admin signature.

**Request:**
```json
{
  "peer_id": "12D3KooW...",
  "role": "member",
  "owner": "alice",
  "signature": "<hex>"
}
```

---

### `POST /circles/<id>/members/reject`

Reject a pending member.

**Request:**
```json
{ "peer_id": "12D3KooW..." }
```

---

## Proposals

Proposal endpoints read and update the local proposal store in the workspace.
Proposal IDs can be full IDs or unambiguous prefixes.

### `GET /circles/<id>/api/proposals`

List proposals.

---

### `GET /circles/<id>/api/proposals/<proposal_id>`

Return proposal metadata plus per-file diffs.

---

### `POST /circles/<id>/api/proposals/<proposal_id>/accept`

Accept a legacy or explicitly staged pending proposal. Normal live-workspace
changes are recorded as accepted history immediately.

**Response `200`:**
```json
{ "status": "accepted", "proposal_id": "..." }
```

---

### `POST /circles/<id>/api/proposals/<proposal_id>/reject`

Reject a legacy or explicitly staged pending proposal and reverse-apply its
changes. Normal live-workspace changes use the revert endpoint instead.

---

### `POST /circles/<id>/api/proposals/<proposal_id>/revert`

Revert a previously accepted proposal.

---

## Circle Lifecycle

### `POST /circles/<id>/stop`

Stop a running circle (cancel its P2P swarm and tasks). The circle remains configured and can be restarted.

**Response `200`:**
```json
{ "status": "stopped" }
```

---

### `POST /circles/<id>/start`

Start a stopped or newly-enabled circle without restarting the daemon.

**Response `200`:**
```json
{ "status": "started" }
```

---

### `POST /circles/<id>/api/enable`

Enable a disabled circle and start it if possible.

**Response `200`:**
```json
{ "status": "enabled" }
```

---

### `POST /circles/<id>/api/disable`

Disable a circle and stop it.

**Response `200`:**
```json
{ "status": "disabled" }
```

---

### `POST /circles/<id>/api/leave`

Stop a circle and remove the local circle config directory.

**Response `200`:**
```json
{ "status": "left" }
```

---

### `POST /circles/<id>/api/invite`

Generate a 7-day invite for a circle. The response contains `invite_uri` and a
`connectivity` object describing embedded peer, relay, and rendezvous addresses.

---

## Device-Local Routes

These routes are not scoped to a circle. They edit this machine's local
configuration and identity.

| Method | Path | Purpose |
|--------|------|---------|
| `GET` | `/api/identity` | Read device label, user handle, and user-key status |
| `POST` | `/api/identity` | Set device label and/or user handle |
| `POST` | `/api/identity/create-user` | Create a user identity and return a mnemonic |
| `POST` | `/api/identity/link` | Link this device to a user identity |
| `GET` | `/api/agent-config` | Read local agent launch config |
| `POST` | `/api/agent-config/reaction` | Set mention reaction: `pull` or `push` |
| `POST` | `/api/agent-config/agents` | Add or replace an agent launcher |
| `POST` | `/api/agent-config/agents/remove` | Remove an agent launcher |
| `POST` | `/api/init` | Frontend helper for `enox init` |
| `POST` | `/api/enter` | Frontend helper for `enox enter` |

---

## Events

### `GET /circles/<id>/api/events`

SSE stream of all circle events. Keep the connection open to receive real-time updates.

**Response headers:**
```
Content-Type: text/event-stream
Cache-Control: no-cache
```

**Frame format:**
```
data: <json>\n\n
```

**Event types:**

| `type` | Fields | Trigger |
|--------|--------|---------|
| `file_updated` | `path` | A workspace file changed |
| `file_deleted` | `path` | A workspace file was deleted |
| `lock_acquired` | `path`, `agent_id` | File lock acquired |
| `lock_released` | `path`, `agent_id` | File lock released |
| `task_created` | `task_id` | New task created |
| `task_claimed` | `task_id`, `agent_id` | Task claimed |
| `task_done` | `task_id` | Task marked done |
| `presence_changed` | `agent_id` | Agent presence updated |
| `member_added` | `peer_id` | Member added to circle |
| `member_removed` | `peer_id` | Member removed from circle |
| `message_posted` | `message` | Chat message posted |
| `agent_mentioned` | `agent_id`, `message` | An agent was @mentioned in chat |
| `proposal_created` | `proposal_id` | Workspace change captured as a proposal |
| `proposal_updated` | `proposal_id`, `status` | Proposal status changed |

**`message` object shape:**
```json
{
  "id":       "b3e4f1a2-...",
  "agent_id": "mymac-KRhAf4ug",
  "text":     "hello @bob",
  "mentions": ["bob"],
  "ts":       1747308000
}
```

---

## WebSocket

### `GET /circles/<id>/ws/yjs`

Yjs sync WebSocket for collaborative document editing. Connect with a standard Yjs provider (e.g. `y-websocket`). See [protocol.md](protocol.md) for the sync protocol.
