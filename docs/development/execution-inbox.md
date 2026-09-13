# Durable agent execution inbox

Each device owns an execution inbox for each Circle, separate from replicated chat
and separate from the agent's ACP conversation. Different agents can execute
concurrently; turns of one agent stay ordered, with ACP memory keyed by agent and Circle.

## Admission and reconnect

`execution_inbox.json` stores requests and lifecycle outcomes under the Circle's
local state directory. It is not replicated. A stable activation timestamp is
written the first time this version starts a Circle. Explicit mentions and replies
after that boundary can be admitted when received, even if the device restarted
or newer messages have already arrived. Reconciliation runs at startup, every five
seconds, and after an event-stream gap. Policy and the configured agent allowlist
remain local gates. A missing explicit reply parent is reconsidered after sync.

Chat retention still limits delivery of requests that have never reached this
device: another peer must retain and sync the message (currently up to 30 days and
10,000 persisted chat messages). Once admitted, the inbox retains the request
itself, independently of transcript retention. This is not a sender-side receipt
protocol; seeing a message in chat does not prove its recipient admitted it.

Historical messages before first activation are not automatically replayed. Existing
`handled_mentions.log` entries remain suppression evidence only: the old runtime
marked them before execution, so they cannot establish completion. Matching retained
messages are imported as `legacy_suppressed`, with an explicit retry action.

Reconciliation does not replay ambient observations or invent new recency follow-ups.
Ambient participation is offered only on live messages less than 30 seconds old;
its existing authorship, length and quiet-period gates still apply. Admitted
follow-ups are durable, but the current recency window is not an offline work inbox.
Explicit mentions or replies are the reliable form for offline requests.

## Lifecycle

- `pending`: saved before the worker is notified. Survives restart.
- `running`: saved immediately before invoking the agent.
- `completed`: the invocation and any text-reply publication returned successfully.
  This does not assert that the user's requested work passed independent review.
- `failed`: the invocation or reply publication failed; no automatic retry.
- `interrupted`: the daemon restarted with a run still marked running. No automatic
  retry, because external effects may already have happened.
- `cancelled`: a chain stop, targeting change, or relay budget prevented execution.
- `expired`: queue overflow displaced the request, or a pending ambient observation
  was discarded on restart.

Retry or cancel through the Agent delivery panel. Retry creates a new attempt and
retains the previous outcome; the UI asks the user to acknowledge possible duplicate
effects. Only pending attempts can be cancelled. Transcript replay never revives a
terminal request.

Atomic snapshot replacement and file synchronization precede acknowledgement and
launch. A process-level file lock prevents two inbox owners from executing the same
Circle. A persistence error stops further execution until restart/recovery. A
corrupt or unsupported inbox is reported as an error and never silently reset.
Terminal dedup records and cancellation tombstones are retained; compaction needs a
future explicit retention protocol, not a timeout that could revive old work.

## Bounds and policy

The existing four-pending-turn limit per agent is retained; a 64-pending-turn limit
also bounds the whole Circle, with 256 pending requests across active device inboxes.
Per-agent/Circle overflow expires the oldest affected pending request; device
overflow expires the new admission with a visible reason. Running requests are never
displaced. Pending jobs, terminal outcomes, and last-known remote receipts are shown
in the Agent delivery panel.

Jobs contain the source request, not a saved executable command or actor token.
The worker reloads policy and the command before launching. Pull mode or removal
from the allowlist pauses pending requests. A retargeted device cancels the request.
Only one resolved job per message/agent is admitted, even if two handles name it.

Relay budget charges are persisted atomically with admission. Duplicate delivery
does not spend again; restarting does not reset the local budget. Wire depth and
local per-root ceilings remain separate from a globally coordinated total.

Stop-chain decisions no longer expire after one hour. They are saved in durable
control state and applied before a queued turn launches, including root turns that
have not started. A busy control document defers launch until a later check. A stop
does not interrupt a process already running. All participating devices must run the
updated version to preserve the durable-stop guarantee across their restarts.

## Inspection

`GET /circles/<id>/api/chat/executions` returns this device's admission boundary and
run summaries, newest admission first. Query parameters:

- `message_id`: filter by originating chat message.
- `limit`: page size, default 50, clamped to 1–200.
- `before`: run ID returned as `next_cursor` to retrieve the next older page.

The response includes `peer_id`, `activated_at`, `runs`, and `next_cursor`. Each run
has `run_id`, `message_id`, `agent_id`, `status`, `detail`, `admitted_at`, `updated_at`,
`relay_root`, `reply_to`, and `ambient`. It excludes prompt bodies, commands and
credentials. Reading status never opens the inbox for ownership or performs crash
recovery. This endpoint reports the receiving device's full local history.

`GET /circles/<id>/api/chat/deliveries` returns up to 100 recent sanitized receipts
from replicated control state, plus the local `peer_id`. Remote receipts are last
known states, not a live connectivity guarantee. Unreceived requests have no receipt.

`POST /circles/<id>/api/chat/executions/<run-id>` accepts `{"action":"retry"}` or
`{"action":"cancel"}`. Mutations use the owning inbox, persist before acknowledgement,
and apply only on the recipient device. Launch-time policy and stop checks still apply.

## Parallel execution and attribution

`max_concurrent_runs` in `agents.toml` defaults to 4 (range 1–32); set 1 for serial
operation. The device settings UI exposes this limit. Restart the daemon after
changing it. FIFO daemon permits coordinate Circles, and OS slot leases also cover
manual launches. Each agent/Circle additionally has an exclusive OS conversation
lease. Independent runs have separate `managed_runs/<run-id>.json` records and
`ENOXIAN_RUN_ID`; chat-triggered actor tokens carry the same identity.

Native ACP writes journal before/after blobs and run identity before proposal
processing. Each operation produces its own proposal, retaining intervening edits
and pending review for ambient work. Write capture and proposal commits are ordered
across local processes. Unmediated shell/editor changes remain unknown; timestamps
are never sufficient to assign a verified writer. Mixed interactive/native paths
are conservatively treated as unknown for their unevidenced portion.

Locks use canonical workspace paths and run/device ownership. Local check/acquire
is one control transaction. Native daemon hooks reject another run's lock and show
lock-wait detail; completed runs release only their own locks. Independent clients
must still follow the cooperative lock protocol. A rename requires coordinating
both paths; a path lock is not an inode lease or OS-user isolation. Partitioned peers
cannot obtain globally linearizable ownership from a CRDT alone.

Run records retain child PIDs; surviving processes prevent overlapping recovery.
Unix agents have dedicated process groups for cleanup; Windows uses process lookup
and tree termination. Legacy singleton metadata is preserved, but the old format
has no PID to establish survival. PID reuse can conservatively delay recovery.
Exactly-once external effects and automatic retry after uncertain execution are
not promised.

## Conversation context

New messages carry defaulted `thread_root`; generated answers set `reply_to` to the
trigger. These fields route/display context and never create new ACP conversations.
The default recency window is now zero; explicitly configured values are preserved.

Context delivery advances only through the page actually supplied to the agent,
never to its outgoing reply. Remaining room history is retrieved through
`GET /circles/<id>/api/chat?after_id=<message-id>&limit=100`. Start without `after_id`
for older history. Explicit ancestors are included separately. A failed ACP resume
is identified in the next prompt with fresh room context and history retrieval
instructions; missing private provider memory is not presented as preserved.

