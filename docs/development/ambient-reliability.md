# Ambient reliability: making "read the room" actually fire

Companion to [engagement.md](engagement.md) §2, which specified ambient
engagement and is the reference for *what* it is. This spec is about why the
shipped version does not fire when it should, why it goes silent when it fails,
and what it tells the agent when it does run.

Nothing here is built yet.

## 0. The short version

Ambient engagement is admitted by comparing the **author's wall clock** against
**this device's wall clock**. That is the root defect: it makes a local policy
decision from a remote, unsynchronised, untrusted number. Every "the agent
wasn't triggered" report that is not simply "it was never configured" traces
back to it.

Three changes, in dependency order:

1. **Replace the clock gates with a durable observation ledger** (§2). A message
   is eligible because this device has never decided about it, not because it
   was authored recently. Delay stops mattering.
2. **Stop treating a failed turn as an answer** (§3). A crashed adapter
   currently consumes the room's only chance to respond to that message.
3. **Make silence legible and the prompt honest** (§4, §5). A failure that
   vanishes after 45 seconds is indistinguishable from nobody caring, and an
   ambient prompt that opens with `REQUEST from suzy (@mention)` is lying to the
   model about why it was woken.

## 1. The trigger is already an event. The gate is not.

### 1.1 What the path does today

The design is sound and already has the shape you would want:

```
CircleEvent::MessagePosted            (the event)
  → admit_message                      (synchronous policy)
    → offer_ambient → dispatch         (selection)
      → Inbox::admit                   (durable queue, capacity-bounded)
        ─────────────────────────────  async boundary
        → run worker → run_next_cancellable → react
```

`Inbox` is the async boundary, and it is a good one. It is on disk, it survives
restart, it bounds per-agent and per-device concurrency, and a turn that has to
wait for a device permit stays `Pending` and runs later. **Execution latency
already cannot lose a trigger.** That half of the architecture does what you
described.

Admission is where it breaks, and only for ambient.

### 1.2 Three wall clocks in one local decision

`ChatMessage.ts` is stamped by the **poster's** machine
([api/chat.rs:452](../../src/api/chat.rs)). Three predicates in the admission
path compare it against this device's `Utc::now()`:

| Where | Predicate | Scope |
|---|---|---|
| [reaction.rs:191](../../src/agent/reaction.rs) | `message.ts < inbox.activated_at()` → drop | **all** admission |
| [reaction.rs:64](../../src/agent/reaction.rs) | `live = message.ts >= now - 2` at loop start | ambient + follow-up |
| [reaction.rs:255](../../src/agent/reaction.rs) | `message.ts >= now - 30` | ambient only |

Consequences, all silent:

- A peer whose clock is a minute slow can never trigger an ambient turn here.
  Ever. Not "after a delay" — never, because its messages are born stale.
- A peer whose clock is behind this device's `activated_at` can never trigger
  **anything** here, including an explicit `@mention`. This is the most severe
  instance and it is not ambient-specific.
- A peer that was offline for a minute and then syncs has its messages arrive
  with old timestamps. They are correct timestamps. They are discarded anyway.
- The daemon's own event loop blocking for 30 seconds — a long `transcript()`
  under contention, a slow `spawn_blocking`, a laptop suspend — drops every
  ambient turn in that span.

None of these produce a log line above `trace!`, and two of them produce no log
line at all.

### 1.3 What the clock was actually defending against

It is worth being precise, because the defence is real and any replacement must
keep it.

P2P sync replays the entire chat history as fresh `MessagePosted` events. This
is stated plainly in [handled.rs](../../src/agent/handled.rs)'s module doc: on
reconnect, "P2P sync replays the entire chat history as fresh CRDT updates, so
without a durable guard every past mention re-launches its agent on every
restart." Join a Circle with 3000 messages and a naive ambient path fans out
3000 model turns.

For **addressed** mentions the defence is a durable dedup set — `handled_mentions.log`
plus `Inbox` returning `Admission::Duplicate` on a repeated
`(message_id, mention_key)`. That mechanism is clock-free and correct, which is
why addressed work is replayed safely by `reconcile_requests` every five
seconds and why an `@mention` that arrives an hour late still runs.

Ambient has no equivalent, because its dedup key does not exist before the
decision is made: the decision *is* "which agents get offered this". So the
implementation reached for a clock instead. The one-shot guard at
[reaction.rs:404](../../src/agent/reaction.rs) is a partial, in-inbox
substitute, and §3 is about how it overshoots.

### 1.4 The invariant

> An ambient turn is offered for a human message **the first time this device
> decides about it**, whenever that happens, and at most once.

"First time this device decides" is a local, observable, monotonic fact.
`message.ts` is none of those things. The invariant makes arrival latency
irrelevant by construction, which is the property you asked for.

## 2. The observation ledger

> **Shipped (Phase C), except §2.4.** `agent::ledger::AmbientLedger` is the
> durable set; `reaction::drain_ambient` replaces the per-event `offer_ambient`;
> `ambient_backlog_tail` (default 1) collapses a backlog to its tail. The
> `ts >= now - 30` gate and the in-inbox one-shot guard are both gone. §2.4
> (retiring `activated_at`) is still Phase E.

### 2.1 Shape

A per-Circle durable set of message ids this device has already made an ambient
decision about, with the decision recorded for diagnosis:

```
circle_dir/ambient_decisions.log
  <message_id> <decision> <unix_ts>
```

where `decision` is one of `offered`, `skipped:<reason>`, `backlog`,
`pre-activation`. Append-only, one line per message, same shape and failure
posture as `handled_mentions.log`: a lost write costs at most one duplicate
offer, never a lost one.

Compaction: on load, drop any id no longer present in the transcript. The
ledger is then bounded by transcript size, not by uptime.

*As shipped, the line is `<message_id> <unix_ts> <decision>` — the decision last,
because a skip carries its reason and reasons contain spaces. Putting it at the
end makes it the rest of the line, so nothing needs escaping.*

*Two cases seed the ledger rather than draining into it. On **first run** in an
existing Circle, the whole transcript is recorded as `pre-activation`; otherwise
upgrading to this build would collapse years of chat into a turn nobody asked
for. When **no listener is configured**, undecided messages are recorded the
same way rather than left to accumulate — which keeps the drain O(new messages)
and means switching a listener on later does not retroactively answer
everything said while none was on.*

This replaces the `ts >= now - 30` gate outright and subsumes the existing
in-inbox one-shot guard, which becomes redundant and should be deleted rather
than kept alongside — two dedup mechanisms disagreeing is how the `~ambient:`
marker import in [inbox.rs:145](../../src/agent/inbox.rs) came to exist.

### 2.2 Admission becomes a drain, not a per-event decision

Today `offer_ambient` runs once per `MessagePosted` event. Instead, the event
marks the Circle dirty and the loop *drains*: compute the set of human-authored
messages in the transcript that have no ledger entry, and resolve all of them in
one pass.

Draining rather than reacting is what makes §2.3 expressible at all, and it
picks up a second thing [engagement.md §2.5](engagement.md) asked for and never
shipped: "debounce a burst of messages into one turn". Three lines typed in
four seconds — "hey" / "so about the API" / "the retry thing is wrong" — drain
together as one undecided set, and the tail carries the turn. Today they are
three independent events, the first two die on the length floor, and the third
gets a turn with no idea the other two were part of the same thought.

The drain runs on the existing five-second reconcile tick plus a short debounce
after each event (say 750ms of quiet, capped at 3s). No new timer loop.

### 2.3 Backlog collapse, not backlog discard

The drain set can be large: first sync, a long offline period, a peer catching
up. Spending a turn on each is exactly the cost blowup §2.5 worried about.

The rule is **collapse to the tail**:

- Order the undecided set by CRDT array order — `state.transcript()` already
  returns Yrs insertion order, which every peer agrees on and which involves no
  clock at all. (Deliberately *not* `ts` order; the reconcile path's
  `sort_by(ts)` is a separate, smaller clock dependency worth removing too.)
- The last `ambient_backlog_tail` (default **1**) eligible messages get a real
  offer.
- Every other undecided message gets a ledger entry `backlog` and no turn.

*Eligibility is decided before the tail is taken, not after: a backlog whose
newest line is "ok thanks" would otherwise spend its one turn there while a real
question sat behind it.*

The property this buys, stated as the user-visible promise:

> However late a message arrives, and however long the daemon was down, the
> **most recent thing said in the room** is always read.

That is strictly better than today in both directions. Today a 31-second-old
message gets nothing; under this rule it gets a turn. Today a 3000-message
backfill also gets nothing, which happens to be affordable but is accidental,
and one clock skew away from being 3000 turns.

A drain of size 1 — the overwhelmingly common case — is unaffected by all of
this.

### 2.4 Retiring `activated_at`

`activated_at` is the same defect one layer up and it damages addressed work,
so it should go the same way. At first activation, seed the ledger with every
id currently in the transcript, marked `pre-activation`. Afterwards "was this
message posted before the agent was switched on" is answered by set membership
rather than by comparing two machines' clocks.

Cost is one O(n) pass at first activation and n ledger lines; the compaction
rule in §2.1 keeps it proportional to the transcript. The `activated_at` field
stays in the snapshot for the `/api/execution` response and for display, but
stops being consulted for admission.

This is separable from §2.1–2.3 and can land after them. It is listed here
because it is the same bug, and because "my @mention was ignored on that
machine and I never found out why" is a worse symptom than anything ambient
does.

### 2.5 What does not change

The cheap gates in [ambient.rs](../../src/agent/ambient.rs) stay exactly as they
are: human-authored only (§2.1 — the termination property), the length floor,
the 30-second per-agent quiet period. Those are content and cost policy, not
liveness, and they are honest about being crude. `spoke_recently` should take
the drain's wall-clock `now` rather than `message.ts`, since it is asking a
question about *this device's* recent past.

## 3. A failed turn is not an answer

> **Shipped (Phase D).** A message whose every attempt failed rejoins the
> drain's candidates and rotates to the next listener; `ambient_max_attempts`
> (default 2, per Circle) bounds it. `ambient_turn_timeout_secs` (default 180,
> device-wide) stops a conversational aside holding a device permit for half an
> hour. §3.3 was decided as recommended — PASS settles.

### 3.1 The one-shot guard overshoots

[reaction.rs:404-411](../../src/agent/reaction.rs):

```rust
if previous.iter().any(|e| e.request.ambient && e.request.message.id == message.id) {
    return;
}
```

Any prior ambient entry for the message ends the matter, whatever its status.
So `Failed`, `Expired`, `Cancelled` and `Interrupted` all consume the room's
single chance to respond. The ways that happens are not exotic:

- the ACP adapter fails to start, or the provider errors, or the 30-minute
  `session/prompt` timeout at [acp.rs:373](../../src/agent/acp.rs) fires;
- `launch_rejection` returns `ambient participation disabled` or
  `recipient device changed` because config moved while the turn queued;
- the daemon restarts and every `Pending` ambient entry becomes `Expired`
  ([inbox.rs:185](../../src/agent/inbox.rs));
- `MAX_PENDING_PER_DEVICE` is exceeded during recovery.

`run_next_cancellable` writes `Status::Failed` and publishes a `Skipped`
activity, and that is the end of it. `reconcile_requests` cannot help: it
explicitly replays only messages carrying a mention or `reply_to`
([reaction.rs:159](../../src/agent/reaction.rs)), with the comment "never stale
room observations".

### 3.2 Re-offer

Under the ledger the fix is small, because the ledger records *decisions*, not
*attempts*. A message is settled when it has a non-failure terminal outcome:

- `Completed` (including PASS — see §3.3) → settled.
- `Running` / `Pending` → in flight, not re-offered.
- `Failed` / `Expired` / `Interrupted` → **unsettled**; the message returns to
  the undecided set.
- `Cancelled` → **settled**. The spec had this wrong: a cancellation is a
  decision, not a failure. "Another agent answered first" (§5.2), "cascade
  stopped" and "ambient participation disabled" are all this device choosing
  not to run, and retrying a decision undoes it.

Selection already sorts least-recently-offered-first
([reaction.rs:418](../../src/agent/reaction.rs)), and the failed agent's entry
is the most recent, so it sorts last. Rotation to the next listener is free.

Bound it with `ambient_max_attempts` (default **2**) per message, counted from
ledger entries, so a message that kills every adapter in the room costs two
turns and not N. On exhaustion the ledger records `skipped:attempts-exhausted`
and §4 surfaces it.

*As shipped, attempts are counted from **inbox entries**, not ledger entries.
The inbox is already the durable record of what was attempted — that is what it
is for — and a parallel counter in the ledger is state that can disagree with
it. Counting entries rather than distinct agents also bounds the restart loop
below.*

*Exhaustion is announced in the room, not only in the panel. This is the case
§4.1 reserved the transcript for — "the room will not be answered, and here is
why" — and it is the point at which that becomes true. Phase A announced every
individual failure, which was right when a failure really was the end of the
message and became wrong the moment this phase landed; `announce_failure` is now
addressed-only, with `announce_unanswered` for this.*

`Interrupted` deserves a note: a daemon restart mid-turn is the one case where
the agent may have already done work the user can see. Re-offering to a
*different* agent is right; re-offering to the same one is a retry and should
stay manual (`Inbox::retry` already exists).

*`Expired` turned out to need the opposite treatment, which the spec missed.
A turn that expired on restart never started, so its agent did not try and fail
at anything — and in a one-listener room there is nobody to rotate to, so the
rule above would settle the message as "every agent tried" when none had. An
expired run is therefore requeued to the same agent through `Inbox::retry`,
which is the one same-agent retry the drain performs. `Interrupted` keeps the
manual-only rule, because that run was executing.*

### 3.3 Is PASS an answer?

Today yes: PASS reaches `Completed`, so the message is settled and the other
listeners never see it. Three ambient agents, the first passes, the room is
silent.

[engagement.md §2.3](engagement.md) only reasoned about pile-on — too many
agents answering one line. Under-response is the symmetric failure and the spec
does not mention it.

**Recommendation: keep PASS settling, and solve the underlying want elsewhere.**
"Nobody answered" is usually `ambient_responders = 1` plus a length floor, not
PASS. Making PASS cascade turns one cheap decline into N model turns on exactly
the messages the heuristics already judged marginal — it inverts the cost model
§2.5 was built around. If real Circles show PASS-then-silence as a common
outcome, the cheaper lever is raising `ambient_responders`, which fans out in
parallel and is already configurable.

Left as an open question in §8 rather than decided, because it wants data.

### 3.4 An ambient turn is not a work order, including in its timeout

`react()` sends ambient and addressed turns down one `driver::launch_cancellable`
with one 30-minute ceiling. A conversational turn nobody asked for should not be
able to hold a device permit and a conversation lease for half an hour while
addressed work queues behind it.

Add `ambient_turn_timeout_secs` (default **180**). On expiry the turn is
`Failed` with `ambient turn timed out`, which §3.2 then treats as unsettled.
The existing `CancellationToken` plumbing carries this; no new mechanism.

*The `CancellationToken` does not in fact carry it: `launch_cancellable` only
uses the token to bound `DeviceLease` acquisition, and nothing cancels a run in
flight. Shipped instead as a per-session limit on the ACP client's
`session/prompt` call, which is where the 30-minute ceiling already lived —
`AcpSession::set_prompt_timeout`, set after the handshake so `session/load`
keeps its own. Device-wide rather than per Circle, because it bounds a shared
device resource; §8.4's question therefore stands only for the per-agent half.*

## 4. Silence must be legible

> **Shipped (Phase A).** Terminal failures post one throttled system line
> (`reaction::announce_failure`), and every admission skip is recorded with its
> reason in `agent::decisions::AdmissionLog` and surfaced under "Not picked up"
> in the activity panel, alongside a readiness summary for the standing
> configuration faults. Two things this section asked for were already there and
> were left alone: the execution inbox is durable and already exposed, and
> `Inbox::retry` was already wired to a "Try again" button.

[engagement.md §2.3](engagement.md) already said this — "a pass is not the same
as a crashed adapter... or users will not trust the Circle" — and the shipped
version only half does it. `ChatActivity` has a **45-second TTL**
([api/chat.rs:25](../../src/api/chat.rs)). A turn that fails after thirty
minutes flashes an indicator for forty-five seconds, most likely at nobody, and
leaves nothing in the transcript.

### 4.1 Terminal outcomes persist

Terminal ambient outcomes stop being ephemeral. Two pieces:

- **Execution inbox** is already durable and already exposed at
  `/api/execution`. Surface unsettled failures in the UI with the existing
  `Inbox::retry` as a one-click action. This is the diagnostic surface.
- **Transcript**, only for the case that actually needs it: the room asked
  something, every attempt failed, and no agent will speak. Post one
  `Author::System` line, collapsible, naming the agent and the reason.
  `Trigger::System` already exists ([api/chat.rs:420](../../src/api/chat.rs))
  and is unused by this path.

Not for PASS, not for the length floor, not for per-attempt failures — only for
"the room will not be answered, and here is why". Anything more and the system
line becomes noise people learn to ignore, which is the same failure as
invisibility.

### 4.2 Every skip has a reason

`offer_ambient`'s skip is `tracing::trace!`
([reaction.rs:399](../../src/agent/reaction.rs)). `dispatch`'s first two returns
— wrong `Reaction` mode, agent not in `cfg.agents` — log nothing at all
([reaction.rs:298](../../src/agent/reaction.rs)).

Every ledger entry already carries its reason by construction (§2.1). Expose the
last N decisions on `/api/execution` so "why did nothing happen" is answerable
without a debug build. This is most of the value of the whole spec for anyone
operating a Circle.

## 5. The ambient prompt contradicts itself

> **Shipped (Phase B).** `context::Framing` splits the addressed prompt from the
> overheard one: an unaddressed turn now opens `MESSAGE overheard in circle …`,
> the standing brief says it is a group room rather than claiming an `@mention`,
> and `ambient_instruction` drops the denial it only needed in order to undo the
> old header. Co-listeners are named there rather than in the header (§5.2), the
> queue cancels a turn another agent has already answered, and the trigger
> message's attachments are rendered with their fetch URLs (§5.3). The addressed
> prompt is byte-for-byte unchanged, which a test pins.

`compose()` emits, unconditionally
([context.rs:324](../../src/agent/context.rs)):

> `REQUEST from suzy (@mention) in circle "X". Respond only to this:`

and the standing brief adds ([context.rs:369](../../src/agent/context.rs)):

> `You were woken by an @mention in the circle's chat.`

then `react()` appends `ambient_instruction()`:

> `You were not addressed. ... If you have nothing worth adding, reply with exactly PASS`

So the prompt asserts the agent was mentioned, frames the text as a REQUEST
addressed to it, instructs it to respond, and then denies all of that in a
trailing paragraph. The leading frame is the stronger signal, and on a resumed
session the model has been trained across prior turns that `REQUEST from` means
"answer this". PASS is being asked for from a position of contradiction, which
is a plausible part of why ambient agents feel both over-eager and unreliable.

### 5.1 An ambient frame

`compose()` takes the turn kind. For ambient:

```
OVERHEARD in circle "X". suzy posted this to the room; it is addressed to
no one in particular and you were not named.
```

and `standing_brief` conditions its "woken by an @mention" sentence on the same
flag. The trailing `ambient_instruction()` then reinforces rather than
contradicts.

### 5.2 What else the turn should know

Three facts the selection logic already has and throws away:

- **Who else was offered this.** `offer_ambient` computes the list before
  dispatching. Passing it in ("codex was also offered this message") is the
  cheapest available brake on pile-on, and the spec flagged pile-on without
  addressing it beyond a numeric cap.
- **Whether anyone has answered already.** A turn can sit `Pending` behind a
  device permit for minutes while another agent replies. `build_delivery` does
  include the latest room context, so the information is technically in the
  prompt — but nothing tells the agent to read it as "this may already be
  handled; PASS is fine". Say it explicitly, and re-check for an agent reply to
  this message at the moment the permit is acquired, before transitioning to
  `Running`. If one exists, cancel with `already answered` and settle the
  message. *(Shipped as `launch_rejection`'s "another agent answered first",
  which covers the queue wait, plus a prompt line for the rest of the race — a
  reply that lands after the turn has started. Ambient only: naming an agent
  means you are owed its answer whatever anyone else says.)*
- **That this is a group room.** The brief lists members but never says the
  conversation is multi-party and mostly not about the agent.

### 5.3 Attachments

`skip_reason` deliberately lets an image with no text through
([ambient.rs:96](../../src/agent/ambient.rs)), with a test asserting it
(`an_image_with_no_words_is_still_worth_a_look`). But `task = message.text` is
empty and [context.rs](../../src/agent/context.rs) never references
`attachments` at all. The result is a full ACP turn on an empty REQUEST.

Either render attachments into the prompt (name, mime, and the fetch URL, in the
`<context>` block) or delete the carve-out and its test. Rendering them is
better and is a precondition for the carve-out being anything other than a
token leak.

*Shipped as rendering, via `context::attachment_note` — but after the task text
rather than inside `<context>`, because an attachment is part of what was posted
and not background to it.*

## 6. Smaller defects found on the way

- ~~**Case-sensitive allowlist.**~~ **Fixed in Phase C**, which had to resolve
  listener names against `[agents.*]` anyway: the drain now matches without
  regard to case and carries the configured spelling forward, since
  `cfg.resolve` and the `~ambient:` dedup key both want the exact key.
  Previously: `offer_ambient` filtered
  `settings.ambient.iter().filter(|n| cfg.agents.contains_key(*n))` — exact
  match — while `ResolvedSettings::is_ambient` uses `eq_ignore_ascii_case`
  ([config.rs:203](../../src/agent/config.rs)). Hand-written
  `ambient = ["Claude"]` against `[agents.claude]` silently does nothing. Match
  case-insensitively in both, or normalise on load.
- **Replying to a human drops the message entirely.** A message with `reply_to`
  fails `ambient_route_allowed`; if the parent is not an agent reply,
  `resolve_reply_to` returns `None`. Neither path claims it. Quoting a person
  guarantees no agent response. Ambient should be allowed when the reply-to
  resolves to no agent.
- **Defaults make the feature invisible.** `ambient` is empty and
  `engagement_window_secs` is `0` ([config.rs:158](../../src/agent/config.rs)),
  so out of the box no unaddressed message reaches any agent. That is a
  defensible privacy posture ([engagement.md §2.6](engagement.md)) but it is the
  single most common cause of "the agent wasn't triggered", and the UI should
  say so where the user is looking rather than only in the roster.
- **`~ambient:` is a string-prefix protocol** parsed in three places
  ([reaction.rs:131](../../src/agent/reaction.rs),
  [inbox.rs:145](../../src/agent/inbox.rs),
  [reaction.rs:414](../../src/agent/reaction.rs)). Agent names are not validated
  against `:`. Validate on config load.
- **`ambient_rotate_count` phase is unstable.** `1 + offered_messages.len() % max`
  counts every ambient entry ever in the inbox, which is trimmed and rebuilt
  across restarts, so the rotation phase is not reproducible. Cosmetic, but the
  setting promises a cycle it does not deliver.
- **`HandledMentions::mark_new` is dead in production** — only `contains` and
  `entries` are called, for legacy import. Either the ledger in §2 absorbs this
  file or it should be marked import-only in its module doc, which currently
  describes it as the live dedup mechanism.

## 7. Phasing

Each phase is independently landable and independently useful.

| Phase | Content | Why this order |
|---|---|---|
| ~~**A**~~ | ~~§4 — persist terminal outcomes, expose decisions/reasons on `/api/execution`, wire `Inbox::retry` in the UI~~ **Shipped.** | Nothing else can be diagnosed until failures are visible. Smallest diff, largest immediate payoff. |
| ~~**B**~~ | ~~§5 — ambient prompt frame, who-else-was-offered, already-answered recheck, §5.3 attachments~~ **Shipped.** | Text and plumbing only, no state changes. Independently improves PASS quality. |
| ~~**C**~~ | ~~§2.1–2.3 — observation ledger, drain, backlog collapse; delete the `ts >= now - 30` gate and the in-inbox one-shot guard~~ **Shipped.** | The core fix. Needs A to be verifiable. |
| ~~**D**~~ | ~~§3.2, §3.4 — re-offer on failure, attempt budget, ambient turn timeout~~ **Shipped.** | Builds directly on C's ledger. |
| **E** | §2.4 — retire `activated_at` for admission | Highest blast radius (touches addressed work); land last, behind the tests from C. |
| **F** | §6 — the small defects | Any time; independent. |

## 8. Open questions

1. **Does PASS settle a message?** *Shipped as yes.* §3.3 explains why, but the
   answer should still come from measuring real Circles: what fraction of ambient
   messages end in PASS-then-silence where a human then re-asks with an
   `@mention`? That re-ask is the observable signal of under-response.
2. **`ambient_backlog_tail` default.** 1 is the conservative choice. A Circle
   used asynchronously across time zones might reasonably want the last handful
   of a backlog read as one turn — which argues for feeding the tail *as
   context* with a single turn on the last message, rather than a larger K.
3. **Should the drain concatenate the burst?** §2.2 debounces a burst into one
   turn on the tail message. The other burst lines land in the prompt only via
   the normal `build_delivery` window. That is probably enough; if not, the
   drain could explicitly mark them as "the same thought".
4. **Per-Circle vs per-agent timeout** for §3.4. Currently proposed per Circle
   for consistency with the other ambient settings, but a slow local model and a
   hosted one are not comparable.

## 9. Test plan

Property tests, all clock-free and expressible against the existing fakes:

- A message authored with a timestamp one hour in the past, delivered now, is
  offered an ambient turn. *(the skew case — fails today)*
- A message authored one hour in the **future** is offered exactly once and does
  not re-trigger. *(the other skew direction)*
- A transcript of 500 undecided human messages drained at once produces exactly
  `ambient_backlog_tail` offers, all on the CRDT-last messages, and 500 ledger
  entries.
- The same drain repeated after a simulated restart produces zero further
  offers.
- A failed turn returns its message to the undecided set; the next drain offers
  a *different* agent; after `ambient_max_attempts` the message settles as
  `attempts-exhausted` and one system line is posted.
- A PASS settles the message and no further offers occur.
- An agent reply posted while a turn is `Pending` cancels that turn as
  `already answered`.
- An ambient prompt contains neither `REQUEST from` nor `woken by an @mention`,
  and does contain the names of the other agents offered the same message.
- With `activated_at` retired: a peer whose clock is an hour slow can trigger an
  addressed `@mention` on this device.
