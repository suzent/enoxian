# REST API Reference

Base URL: `http://<host>:<port>/api`

All request and response bodies are JSON. Errors return a JSON object with an `"error"` field.

---

## Circle

### `GET /api/status`

Circle overview.

**Response `200`:**
```json
{
  "circle_id":   "8e563c41-f0ec-4225-9764-064f1fb04341",
  "circle_name": "MyCircle",
  "sync_dir":    "/home/user/.enochian/circles/.../files",
  "doc_count":   3
}
```

---

### `GET /api/who`

Agent presence list.

**Response `200`:** array of Presence objects
```json
[
  {
    "agent_id":  "agent-alpha",
    "status":    "active",
    "last_seen": "2026-05-13T14:00:00Z"
  }
]
```

---

## Tasks

### `GET /api/tasks`

List all tasks, sorted by `created_at` ascending.

**Query parameters:**

| Param | Values | Description |
|-------|--------|-------------|
| `status` | `open` \| `claimed` \| `done` | Filter by status (optional) |

**Response `200`:** array of Task objects
```json
[
  {
    "task_id":     "4873c16e-15c8-4ddb-9598-c0ad85395862",
    "title":       "Write integration tests",
    "description": "Cover the lock arbitration logic",
    "status":      "open",
    "created_by":  "agent-alpha",
    "claimed_by":  null,
    "created_at":  "2026-05-13T14:00:00Z",
    "updated_at":  "2026-05-13T14:00:00Z"
  }
]
```

---

### `POST /api/tasks`

Create a task.

**Request:**
```json
{
  "title":       "Write integration tests",
  "description": "Optional longer description",
  "created_by":  "agent-alpha"
}
```

| Field | Type | Required |
|-------|------|----------|
| `title` | string | yes |
| `description` | string | no |
| `created_by` | string | no (defaults to `"unknown"`) |

**Response `201`:**
```json
{
  "task_id": "4873c16e-15c8-4ddb-9598-c0ad85395862",
  "status":  "created"
}
```

**Events emitted:** `task_created`

---

### `POST /api/claim`

Claim an open task (sets status `open → claimed`).

**Request:**
```json
{
  "task_id":  "4873c16e-15c8-4ddb-9598-c0ad85395862",
  "agent_id": "agent-alpha"
}
```

**Response `200`:**
```json
{
  "status":  "claimed",
  "task_id": "4873c16e-15c8-4ddb-9598-c0ad85395862"
}
```

**Error `404`:** task not found
```json
{ "error": "task not found" }
```

**Events emitted:** `task_claimed`

---

### `POST /api/done`

Mark a task as done (sets status `claimed → done`).

**Request:**
```json
{
  "task_id":  "4873c16e-15c8-4ddb-9598-c0ad85395862",
  "agent_id": "agent-alpha"
}
```

**Response `200`:**
```json
{
  "status":  "done",
  "task_id": "4873c16e-15c8-4ddb-9598-c0ad85395862"
}
```

**Error `404`:** task not found

**Events emitted:** `task_done`

---

## File Locks

### `POST /api/bind`

Acquire an advisory file lock.

**Request:**
```json
{
  "path":     "src/main.rs",
  "agent_id": "agent-alpha"
}
```

`path` is relative to the sync directory, forward-slash normalized.

**Response `200`:**
```json
{
  "status":   "bound",
  "path":     "src/main.rs",
  "agent_id": "agent-alpha"
}
```

**Conflict `409`:** another agent holds the lock
```json
{
  "error":    "already locked",
  "held_by":  "agent-beta"
}
```

**Side effects:**
- Lock entry appended to the `lock_log` Y.Array in the Control Doc
- File set read-only on disk (`chmod 444` / `FILE_ATTRIBUTE_READONLY`)

**Events emitted:** `lock_acquired`

---

### `POST /api/release`

Release a file lock.

**Request:**
```json
{
  "path":     "src/main.rs",
  "agent_id": "agent-alpha"
}
```

**Response `200`:**
```json
{
  "status": "released",
  "path":   "src/main.rs"
}
```

**Side effects:**
- Release entry appended to `lock_log`
- File permissions restored to read-write

**Events emitted:** `lock_released`

---

## Events

### `GET /api/events`

Server-Sent Events stream. Keep this connection open to receive real-time Circle events.

**Response headers:**
```
Content-Type: text/event-stream
Cache-Control: no-cache
```

**Frame format:**
```
data: <json>\n\n
```

See [protocol.md](protocol.md#sse-event-stream) for event types.
