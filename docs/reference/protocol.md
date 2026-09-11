# Protocol Reference

## WebSocket — Yjs Document Sync

```
ws://<host>:<port>/circles/<circle-id>/ws/yjs?path=<relative-file-path>
```

Every route below is circle-scoped: a daemon serves many Circles, so the
Circle's UUID is part of the path. Get yours from `enox status` or
`GET /circles`.

Synchronizes a single Yjs document (Y.Text file) between the server and a client. Uses the **y-sync v1 binary protocol** over WebSocket binary frames.

### Handshake

On connect, the server immediately sends a **SyncStep1** containing its current state vector:

```
Server → Client:  SyncStep1(server_state_vector)
```

The client responds with everything the server is missing, then its own state vector:

```
Client → Server:  SyncStep2(client_diff)
Client → Server:  SyncStep1(client_state_vector)
Server → Client:  SyncStep2(server_diff)
```

After the handshake both sides hold a complete, identical document.

### Incremental updates

After the handshake, either side sends incremental updates as they happen:

```
Either → Other:  Update(raw_v1_update_bytes)
```

The server also forwards updates from other sources (file watcher, other WS clients) as `Update` messages.

### Message encoding

All frames are **binary**. Messages are encoded with yrs `EncoderV1`. The leading bytes identify the message type:

| Byte sequence | Message type |
|---------------|--------------|
| `0x00 0x00` | `SyncStep1` |
| `0x00 0x01` | `SyncStep2` |
| `0x00 0x02` | `Update` |

### Example flow

```
connect → ws://localhost:36521/circles/<circle-id>/ws/yjs?path=src/notes.txt

S→C  SyncStep1([state_vector bytes])
C→S  SyncStep2([diff bytes])
C→S  SyncStep1([client state_vector])
S→C  SyncStep2([server diff bytes])

# Later — user edits the file on disk:
S→C  Update([incremental update bytes])

# Client sends a programmatic edit:
C→S  Update([incremental update bytes])
```

---

## SSE — Circle Event Stream

```
GET /circles/<circle-id>/api/events
Accept: text/event-stream
```

Returns a persistent HTTP/1.1 response with `Content-Type: text/event-stream`. Each event is a JSON payload on a `data:` line, followed by a blank line.

```
data: {"type":"task_created","task_id":"4873c16e-..."}\n\n
```

### Event types

Paths below are relative to `/circles/<circle-id>`.

| `type` | Additional fields | Emitted by |
|--------|-------------------|------------|
| `task_created` | `task_id` | `POST /api/tasks` |
| `task_claimed` | `task_id`, `agent_id` | `POST /api/claim` |
| `task_unclaimed` | `task_id`, `agent_id` | `POST /api/unclaim` |
| `task_done` | `task_id` | `POST /api/done` |
| `lock_acquired` | `path`, `agent_id` | `POST /api/bind` |
| `lock_released` | `path`, `agent_id` | `POST /api/release` |
| `file_updated` | `path` | File watcher (disk write detected) |
| `file_deleted` | `path` | File watcher (deletion detected) |
| `presence_changed` | — | Peer presence updates |
| `member_added` / `member_removed` / `member_pending` | member fields | Membership changes |
| `message_posted` | `message` | `POST /api/chat` |
| `agent_mentioned` | `agent_id`, `message` | An `@mention` in a posted message |
| `chat_activity_changed` | `activity` | `POST /api/chat/activity` |
| `proposal_created` / `proposal_updated` | proposal fields | Proposal engine |
| `workspace_event_appended` | event fields | Workspace event log |

### Delivery semantics

Events are distributed via a `tokio::sync::broadcast` channel (capacity 256). This means:

- **Broadcast** — all connected SSE clients receive every event.
- **No replay** — clients that connect late do not receive past events.
- **Drop on lag** — a slow consumer that falls more than 256 events behind will miss events without error.

### Consuming with `enox watch`

```bash
enox watch
# ◆ Watching circle events (Ctrl+C to stop)...
#   [task_created]  {"type":"task_created","task_id":"..."}
```

### Consuming programmatically

```python
import httpx

url = "http://127.0.0.1:36521/circles/<circle-id>/api/events"
with httpx.stream("GET", url,
                  headers={"Accept": "text/event-stream"}) as r:
    for line in r.iter_lines():
        if line.startswith("data: "):
            event = json.loads(line[6:])
            print(event["type"], event)
```

```javascript
const source = new EventSource(
  "http://127.0.0.1:36521/circles/<circle-id>/api/events",
);
source.onmessage = (e) => console.log(JSON.parse(e.data));
```
