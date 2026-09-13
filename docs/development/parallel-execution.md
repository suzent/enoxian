# Parallel execution and reliable message delivery

Status: implemented in the working tree. This records the original implementation
sequence; current behavior and operational limits are documented in
[Execution inbox](execution-inbox.md). ACP conversations remain per agent/Circle.

## Intended behavior

Each agent remains one continuous participant in each Circle: one ACP conversation
session per `(circle, agent)`. Different agents on a device may work concurrently.
Turns for the same agent remain ordered, even when they belong to different chat
threads. Different devices continue to make their own execution decisions.

A chat thread identifies a request and its replies. A run identifies one execution.
Neither replaces an agent's persistent conversation memory. File IO remains native;
enoxian coordinates intent, attribution, locks, and delivery.

## Original baseline and implementation sequence

| Area | Current behavior / gap | Proposed change |
| --- | --- | --- |
| Reply routing | `reply_to` works, but recency routing guesses the recipient for 180 seconds. Ambient and follow-up paths can both dispatch. | Prefer explicit reply threads, default the recency window to zero in a separately documented migration, preserve existing explicit overrides. Resolve addressed routing before ambient eligibility. Explicit agent mentions override reply targeting. |
| Agent memory | One ACP session per agent per Circle; context updates are capped at 12 messages and the seen mark can advance past intervening messages. | Preserve the session key. Separate delivered context cursor from output message ID; include relevant thread context and offer paginated retrieval of missed room history. Never imply that unseen messages were delivered. |
| Offline delivery | Chat sync is durable; the execution queue is not. Startup cutoff discards old unprocessed mentions. Handled markers are persisted before enqueueing. | Persist a recipient-local inbox with admission, execution, completion, failure, cancellation, and expiry states. Reconcile eligible replicated requests after restart/reconnect. |
| Managed runs | One `managed_session.json` per Circle rejects overlapping agents. | Store records by run ID; retain a distinct ordered conversation session for each agent. |
| Attribution | Watcher changes are assigned using the singleton managed session and timestamps. | Journal supported write evidence by run and operation. Split proposals using evidence; retain unknown/mixed attribution where evidence is insufficient. |
| File coordination | Locks identify agent/device; chmod cannot distinguish processes under the same OS user. | Run-aware ownership, atomic local acquisition, cooperative native hooks, lifecycle cleanup, and visible peer conflicts. Do not describe chmod as per-process isolation. |
| Scheduler | One worker per Circle; four pending turns per agent. | Concurrent workers for different agents, one active turn per agent/Circle, device-wide concurrency cap and aggregate pending cap, fair scheduling. |
| Chain controls | Stop/budget checks happen at enqueue time; queued turns can still run after a stop. | Cancel pending runs by root and recheck immediately before launch. Keep stop-chain distinct from interruption of an active process. |
| Visibility | Activity is message/agent based; ambient metadata is advertised but absent from roster rendering. | Show delivery, queue, run, lock-wait and terminal states; show ambient listeners; preserve recipient device identity in reply UI. |

## Implementation sequence

### 1. Exclusive addressed routing and explicit reply UX

Initial change: resolve follow-up targets without first filtering out remote devices.
Suppress ambient routing for local or remote follow-ups, explicit mentions, and
explicit replies, including replies whose parent has not synced yet. Only the
resolved target device executes. Preserve existing recency settings in this first
change to avoid silently changing the running Circle's configuration.

Next: carry thread/root metadata in the defaulted chat schema and generated replies;
make reply controls the primary composer path. Change the default recency window to
zero with migration tests. Unresolved explicit replies must remain pending or show
an unresolved target, never fall through to an unrelated agent.

Verification: local and remote targets; conflicting reply plus mention; missing
parent; expired/disabled recency; one human message never enters two routing paths.

### 2. Durable execution inbox

Introduce an atomic persisted inbox indexed by `(message_id, resolved recipient)`.
Store the source message, admission reason, reply/thread identity, relay provenance,
run ID, timestamps and state. Never store actor bearer tokens in jobs.

Distinguish pending, running, completed, failed, cancelled, expired, and interrupted
outcomes. A durable claim must precede launch; terminal outcomes follow confirmed
completion. Deduplication is not evidence of completion. Retry only failures known
to have happened before execution; a crash during arbitrary tools leaves an
interrupted run requiring explicit retry, since replay can duplicate side effects.
Do not promise exactly-once execution of external tools.

Reconcile newly synced messages independently of transcript position. Introduce a
persisted activation boundary at migration to avoid launching the entire historic
chat log. Requests sent after that boundary while a recipient is offline become
pending when received. Surface expiry and cancellation explicitly; do not silently
replace the startup cutoff with an arbitrary short TTL. Re-evaluate local policy,
recipient identity, parent availability and stop markers on admission and launch.
Do not replay ambient observations automatically as offline work orders.

Migration: import legacy handled entries as legacy-suppressed, not completed; they
cannot establish whether execution succeeded. Surface a manual retry path. Future
jobs obtain full lifecycle evidence. Persist queue changes atomically before UI
acknowledgement; reconcile after event-stream lag as well as reconnect.

Verification: mention while target is down, later chat after the mention, restart
while pending, crash while running, duplicate sync, missing parent arriving later,
policy change while offline, and cancelled/expired requests.

### 3. Multiple managed execution records

Replace the singleton with records keyed by run ID and an atomic local registry.
Persist agent/Circle, trigger, relay root/path, origin, process ownership, start/end
and proposal-consumption state. Carry run identity through actor tokens and managed
process environment. Keep ACP memory keyed by `(circle, agent)`.

Recover runs individually. Check actual process ownership/liveness rather than
assuming a daemon restart killed every descendant. Retain completed records until
write evidence is consumed. Migrate singleton files without losing attribution.
Keep the scheduler serialized during this stage.

Verification: independent finish/recovery; legacy import; delayed filesystem events;
one agent's cleanup cannot clear another run; actor identity survives token renewal.

### 4. Write evidence, proposals, and locks

Start with ACP writes, recording run and operation identity plus before/after blob
hashes around each mutation. Extend supported native adapter hooks without routing
file contents through the coordination CLI. Shell or external-tool writes without
reliable evidence remain unknown/mixed; timing alone is insufficient.

Group proposal operations by run, retaining origin and relay provenance. Maintain
an ordered proposal commit path while execution overlaps. Validate intervening
versions on revert; preserve later unrelated edits and report genuine overlap.
Pending ambient proposals remain live changes marked for review, not staged files.

Add run-aware lock ownership and atomic local check/acquire. Handle canonical paths,
rename, delete, replacement, reentrancy, release and crash recovery. Preserve the
peer-coordination model: concurrent partitioned peers cannot obtain a globally
linearizable lock from CRDT convergence alone. Detect contention and expose it;
stronger isolation is an optional workspace mode, not a claim about existing locks.

Verification: disjoint and same-file edits by two agents plus a human, delayed
watcher events, failed writes, rename/delete, process crash, peer partition, and
selective revert after subsequent edits. Prove attribution before enabling workers.

### 5. Parallel scheduler and chain accounting

Run different `(circle, agent)` queues concurrently under a device-wide permit
limit. Preserve arrival/admission order within an agent, separate messages as
separate turns, and fair selection across Circles/agents. Bound both per-agent and
aggregate backlog. Publish durable overflow outcomes instead of silent loss.

Use persisted run states for in-flight ownership and recovery. Revalidate policy
and stop markers immediately before launch. Remove pending chain branches on stop;
already running processes require a separate interruption operation. Budget charges
must be idempotent per admitted run, including reconnection and restart. Do not claim
a globally strict cascade total from independent per-device counters: retain wire
bounds plus local ceilings and document the distributed limit; widening fan-out
requires a separately designed budget allocation protocol.

Verification: barrier-controlled overlap for different agents; strict same-agent
ordering; fairness; bounded backlog across many Circles; concurrent stop/admission;
no duplicate charges; no unrelated chain cancellation.

### 6. Context delivery, UI, and rollout

Fix delivered-context accounting before increasing room traffic. Each agent's next
turn carries relevant request/thread context and an accurate missed-history cursor;
provide retrieval for the remainder beyond the prompt cap. Do not fork ACP memory.

Render reply destinations, offline/pending delivery, concurrent work, lock waits,
failed/interrupted runs, retry actions, and ambient listener badges. Expose serial
mode for rollback, preserving already-admitted durable jobs. Document configuration,
protocol compatibility and attribution limits; update engagement.md's status claims.

Run deterministic fake-adapter integration tests first, then an explicitly scoped
real-provider smoke test. No deployment or daemon restart is part of the initial
routing patch. Steering/replacement controls are a later extension, not a dependency
of cross-agent parallel work.

## Implementation evidence

The runtime now uses a durable inbox, per-agent conversation leases, per-run records,
shared device slots, and parallel agent workers. Explicit replies and remote targets
exclude ambient dispatch. Stop markers survive restart and cancel queued work.

ACP writes have durable operation evidence and independent proposals; unsupported
writes retain unknown attribution. Run-aware canonical locks and child-process
recovery replace the singleton/timestamp assumptions. Chat context uses delivered
input cursors, explicit thread ancestors, and paginated history. Delivery receipts,
local retry/cancel controls, ambient badges, exact reply recipients, and the serial
fallback are exposed in the UI.

Verification includes process-barrier overlap, same-agent OS exclusion, surviving
process capacity, restart recovery, queue/budget bounds, run-specific lock ownership,
canonical path aliases and replacements, mixed human/agent edits, ambient review,
reply routing, context cursor bounds, remote receipt replication, and retry/cancel
history. A separately invoked Claude ACP smoke test checks conversation continuity
across two isolated runs, without posting to a Circle. Deployment is separate from
these working-tree changes.

The intentionally retained limits are described in the runtime document: cooperative
locks across partitions, unknown attribution for unmediated tools, finite source-chat
retention before receipt, conservative PID-based recovery, and no automatic retry
of uncertain side effects. Steering and record compaction remain separate extensions.

Latest verification: 355 Rust library tests, 13 delivery/delegation integration
tests, and 96 frontend tests pass. The frontend production build, formatting and
diff checks pass. The explicitly invoked real Claude ACP continuity smoke also
passes across two runs. Verification was performed on macOS; no running daemon
was replaced or restarted.
