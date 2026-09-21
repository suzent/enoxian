# Agent engagement

How an agent in a Circle decides that a chat message is *for it*, and what it is
allowed to do once it decides.

**Status: parallel runtime implemented in the working tree.** See
[Parallel execution](parallel-execution.md) and [Execution inbox](execution-inbox.md)
for current implementation, verification, and limits. The design discussion below
retains historical context. Explicit threads now replace recency guessing by default;
explicit recency overrides remain supported.

Background reading, not restated here: [guide/agents.md](../guide/agents.md)
for how mentions, targeting, and the per-device execution gate work, and
[concepts/proposals.md](../concepts/proposals.md) for what happens to files an
agent writes.

## The problem

`@mention` currently does two unrelated jobs at once:

1. **Addressing** — which of the several agents on several devices do I mean?
2. **Turn-taking** — this is a new task; go now.

Job 1 is load-bearing. A Circle is a shared room, and without addressing, every
message would wake every device's agent. Job 2 is ceremony: after an agent has
just replied to you, re-typing `@claude` says nothing the room does not already
know. The result is that a discussion with an agent — the most common thing a
user wants — is the most tedious thing to do.

Everything below separates the two jobs, so addressing can be inferred (or
volunteered) while the execution gate stays exactly where it is.

## Invariants

These hold today and must survive any change here. See
[guide/agents.md](../guide/agents.md) for the full statement of each; the short
form:

- The execution gate is per-device and local (`reaction = "push"` plus the
  agent allowlist). Nothing here moves that decision onto the wire.
- A device never runs an agent addressed to another device.
- An agent's reply never wakes another agent (`fire_mentions = false`). §3
  relaxes this deliberately, and replaces the total ban with an explicit,
  budgeted allowance rather than removing the bound.
- Agent writes land in the live workspace and are recorded as accepted,
  revertible history — the proposal store is an audit and undo layer, *not* a
  staging area ([concepts/proposals.md](../concepts/proposals.md)). §2.4 is the
  one place this spec proposes an exception, and it is a real change.
- Each `(message_id, mention)` pair acts at most once, ever
  (`agent/handled.rs`). Note the key is the mention *string*, not the resolved
  agent.

## 1. Follow-up routing

Goal: a reply to an agent should not need a mention.

### 1.1 Active-engagement window

**As built, this is derived from the transcript rather than stored.** The map
below works on one machine and breaks across several: the composer that must
show "replying to @claude" runs on the *speaker's* device, which would know
nothing about a map held on the agent's device. So the rule is evaluated
identically by every device — *the most recent agent reply to a cascade you
started, within the window, that you have not dismissed.* Both halves this
section asks for turn out to be on the wire already: `relay.root_peer` says who
the agent was replying to, and the reply's `peer_id` is the machine that must
run the follow-up, which is exactly the `scope` described below. Dismissal is
the exception — an intention, not an event — so it lives in the control doc
beside the delegation stop.

The original design, for reference:

When an agent posts a reply, the device that ran it records an **engagement**:

```
(circle, speaker) -> { agent, scope, expires_at }
```

`scope` is the resolved device target from the original mention, carried
forward verbatim — a follow-up must wake the same machine, not whichever
device happens to configure an agent by that name.

`speaker` must be the peer identity, not `ChatMessage.agent_id`. That field is
a display label (`api/chat.rs` posts it as `sender`), so two devices sharing a
label would share an engagement; `peer_id` exists precisely because `agent_id`
is ambiguous (`control/mod.rs`).

Dispatch changes in one place. When a chat message arrives carrying **no**
agent-level mention, look up the speaker's engagement. On a live one, treat the
message as if it had mentioned that agent, with that scope, and with the full
text as the task (there is no mention prefix to strip). Policy, allowlist, and
device targeting are then unchanged.

Durable dedup needs one addition: an implicitly routed message has no mention
string to key on. Use a synthetic key — `{message_id}::@{resolved_agent}` —
and make sure it cannot collide with a real mention of that agent on the same
message, so an explicit mention and a follow-up never both fire.

The engagement ends on the first of:

- **Timeout.** The default is now `0` (explicit replies/mentions only). A configured
  `engagement_window_secs` enables a recency window renewed by each agent reply;
  existing explicit values are preserved.
- **Redirection.** The speaker mentions any agent — including the same one.
  Explicit addressing always wins and re-arms the window.
- **Exit.** The speaker dismisses it in the composer (below). A dismissal
  records *which* reply it dismissed, not just when: chat timestamps have
  one-second resolution, so a dismissal and the reply that re-arms the window
  can share one, and comparing by time alone swallowed it.

Notably it does **not** end because someone else spoke. Engagements are keyed
per speaker, so two people can hold separate conversations with separate agents
in one Circle, and a third person's chatter disturbs neither.

Storage is per-device and ephemeral — in-memory is fine, and a daemon restart
dropping every engagement is acceptable (the next message just needs a
mention).

### 1.2 Composer affordance

Implicit routing that is invisible is a bug. While an engagement is live, the
composer shows what will happen and how to stop it:

```
[ replying to @claude · esc to exit ]
```

Esc clears the engagement locally. This is also the natural surface for §1.3:
when the agent is busy, the same strip reads `claude is working · your message
will be queued`.

### 1.3 Per-agent run queue

Different agents now execute concurrently; turns for the same agent/Circle remain
ordered. A device-wide capacity limit defaults to four. Per-agent OS conversation
leases also cover manual launches, preserving one ACP conversation per agent/Circle.
The durable inbox bounds pending turns at four per agent, 64 per Circle and 256 per
device, with visible overflow outcomes. Pending turns survive restart; interrupted
runs require explicit retry.

The earlier singleton `managed_session.json` and timestamp attribution have been
replaced by per-run records and operation-level ACP write evidence. Ambient writes
remain pending proposals. Other native writes retain unknown attribution rather
than being assigned to whichever process happened to be running.

A rejected alternative: coalescing queued messages into one turn. It reads well
in the transcript but loses the boundary between "and another thing" and a
correction of the first thing.

### 1.4 Reply-threading (phase 2)

The window is heuristic: it guesses from recency, and in a busy Circle it will
sometimes guess wrong. The explicit form is a `reply_to: Option<String>` on
`ChatMessage` — replying to an agent's message routes to that agent with no
timer and no ambiguity, and it is the only thing that works when two agents are
both mid-conversation with the same person.

It needs a schema addition (defaulted, so older peers ignore it) plus real
frontend work, so it lands after the window. They compose: reply-to is explicit
addressing, the window is implicit, and reply-to also re-arms the window.

## 2. Ambient engagement

> **Follow-up spec:** [ambient-reliability.md](ambient-reliability.md) covers
> what the shipped version of this section gets wrong — admission gated on the
> author's wall clock, a failed turn consuming the room's only chance to
> answer, and a prompt that tells an unaddressed agent it was mentioned.

Goal: with several agents in a Circle, let them *see* the conversation and
volunteer, instead of being summoned one at a time.

This is the more interesting idea and the more expensive one. The design below
is deliberately conservative about what an unaddressed agent may do.

### 2.1 Trigger rule

An ambient turn is offered only for **human-authored** messages. Agent replies
and system messages never trigger one.

This is what keeps *ambient* from oscillating: an unaddressed agent's output
cannot become another unaddressed agent's input, so there is no fixpoint to
chase. Ambient is a broadcast, and a broadcast with no authorship rule is a
token spiral.

Explicit agent-to-agent mentions are the deliberate exception, and they buy
their termination differently — see §3. Keep the two apart: the rule here is
"unaddressed turns fire on humans only", not "agents never trigger agents".

Determining authorship needs a durable signal, and neither existing field
supplies it. `agent_id` is a display label — a person may legitimately be
called `codex`. `peer_id` identifies the *device* that posted, which does not
separate a person from the agent running on their machine; both post from the
same peer. So: add an explicit `author: Human | Agent | System` to
`ChatMessage`, defaulted to `Human` for messages from older peers, and set it at
every post site.

### 2.2 Opt-in, per agent, per device

Ambient engagement is configured where every other execution decision lives —
`~/.enoxian/agents.toml`, per agent:

```toml
[agents.claude]
driver = "acp"
command = [...]
engagement = "mention"   # default: only explicit mentions and follow-ups
# engagement = "ambient" # also offered every human message in the Circle
```

Ambient is never a global switch and never a Circle-wide setting. The device
that pays for an agent's tokens is the device that decides whether it reads
idle chat; making this a synced property would let a remote peer spend another
device's budget. This matches the never-synced property `agents.toml` already
has.

### 2.3 The PASS convention

An ambient turn ends in one of two ways: the agent replies, or it declines.

The plumbing for declining already exists — `react()` filters an empty reply
and posts nothing. What is missing is a *convention*, so that "nothing to
add" is distinguishable from a crash. The ambient prompt states it explicitly:

> You were not addressed. You are seeing this because you are a participant in
> this room. If you have nothing worth adding, reply with exactly `PASS` and
> nothing else. Do not explain why you are passing.

`PASS` (alone, case-insensitive, after trimming) suppresses the chat post. The
turn is still recorded in the agent's ACP session, so a passing agent stays
current without speaking.

It should also advance the agent's seen-mark, which needs a small change:
`memory::save_seen` is a no-op when no session record exists, so an agent that
passes on its very first turn would re-read the same history next time.

Two operational consequences worth building for:

- **Silence must be legible.** A pass is not the same as a crashed adapter. The
  activity indicator should distinguish *considered and passed* from *never
  ran*, or users will not trust the Circle.
- **Pile-on is real.** Three ambient agents that all find a message relevant
  produce three replies to one line. Worth a per-message cap on ambient
  replies, or at minimum surfacing the count so a user can turn one off.

### 2.4 What an unaddressed turn may do

An ambient turn is a *conversational* turn, not a work order. An agent that was
not asked to do anything should not be writing files nobody requested.

This is the one place the spec proposes changing the acceptance model rather
than reusing it, and it is more than a flag. Today `AcceptancePolicy::decide`
branches on `TriggerOrigin`, every variant auto-accepts by default, and
`driver::launch` collapses both `Initiator` variants into one `SessionMode`.
Making ambient writes land as `pending` needs a new origin that flows from the
reaction loop through to `decide` — and `pending` has never been used as a
real gate before ([concepts/proposals.md](../concepts/proposals.md) says so
explicitly), so ambient would be its first genuine user. The per-agent override
hook is already anticipated: see the `TODO(M14)` in `proposal/policy.rs`.
(§3.6 explains why a *relayed* turn is not an ambient one and keeps the
existing acceptance path.)

**Both landed.** An ambient turn runs as `Initiator::Ambient`, which maps to
the `SessionMode::AmbientTriggered` that already existed but was unused; the
proposal engine reads that mode and records the proposal as `pending` instead of
`accepted`. That is `pending`'s first use as a real gate. And the ambient prompt
states the narrower rule too — the turn is conversational, so say what needs
doing rather than doing it — because a marker after the fact is weaker than not
writing the files.

Worth being honest about what `pending` does and does not mean here: the files
are still live on disk, as they are for every other proposal. Pending marks them
for review rather than isolating them, and revert remains the undo. It is a
flag, not a staging area.

### 2.5 Cost

This is the part that decides whether the feature is viable, and it should be
measured before it is built.

Every human line in a Circle with three ambient agents is three ACP turns on
three model providers. A five-minute casual exchange between two people —
fifty lines — is a hundred and fifty agent turns, nearly all ending in
`PASS`. That is not a rounding error; it is the dominant cost of running a
Circle, and it is paid on the messages users care about least.

The obvious mitigation is a cheap triage tier that decides whether the
expensive turn is worth taking. The architecture constrains the options:

- **A small local model** is the natural answer and the wrong one here. enoxian
  is deliberately a transport: the user's own CLI owns model selection, auth,
  and billing. Shipping our own inference to gate theirs breaks the property the
  adapter design exists to preserve.
- **A heuristic gate** — no model, no cost. Debounce a burst of messages into
  one turn; skip when the agent replied within the last N seconds; skip
  messages below a length floor; require a question mark or an imperative.
  Crude, but it removes the traffic that dominates the volume.
- **Agent-owned triage** — the honest version of "let the agent decide", where
  the *agent's* runtime does the cheap pass. Out of our control, but worth
  leaving room for.

**Shipped with the heuristic gate**, as recommended: messages under 24
characters with no attachment are skipped, an agent that spoke in the last 30
seconds is left alone. Eligible listeners are selected least-recently-offered first. `ambient_responders`
(default 1) sets how many are offered each message; `ambient_rotate_count` optionally
cycles from one up to that limit. Selection history survives restart, and duplicate
messages do not cause new selections. Shared execution capacity and queue bounds apply. All
three run before any model is asked, so the traffic that dominates the volume
costs nothing. Measure real Circles before reaching for anything smarter.

### 2.6 Privacy posture

Ambient engagement changes what leaves the Circle. Under mentions, casual chat
reaches a model provider only when someone deliberately summons an agent. Under
ambient, **every human line is sent to every configured provider on every
device that opted in** — possibly several vendors simultaneously, for the same
sentence.

That is a defensible trade for a working Circle and an unpleasant surprise for
a social one. It needs to be stated plainly at the point of opt-in, and the
roster must mark ambient agents so *all* peers can see who is listening — not
just the device that configured one.

**Shipped:** `MemberEntry.ambient_agents` is advertised alongside `agents`, so
every peer sees which agents on which devices are reading the room.

## 3. Agent-to-agent delegation

Goal: let an agent hand work to another agent by name, without reopening the
door that `fire_mentions = false` closed.

§2 is about agents *listening*. This is about agents *addressing*. They are not
the same permission and should not be bought with the same switch: ambient is
unaddressed, broadcast, and human-triggered; delegation is explicit, targeted,
and rare. An agent that knows `@codex` is the one with the sandbox, or that a
review should go to a second opinion, should be able to say so and have it
happen — the alternative is a human relaying messages between two programs.

### 3.1 Why it was foreclosed, and what replaces the ban

The original objection was a termination story, not a taste objection. Left
unbounded, `A` mentions `B`, `B` replies mentioning `A`, and the Circle burns
tokens until someone kills a daemon. `fire_mentions = false` is a total ban
because a total ban is the only bound that needs no bookkeeping.

The replacement is a **relay budget**: a small, explicit allowance that a human
message mints and every agent turn spends. When the allowance is gone, mentions
in agent replies go back to being inert text. Termination is then arithmetic
rather than a promise about model behaviour.

### 3.2 The relay chain

Every trigger carries the provenance of the human message that started it:

```
relay = { root: message_id, spent: u8, path: Vec<agent_id> }
```

- `root` — the human message at the base of the cascade.
- `spent` — how many agent turns this cascade has already cost.
- `path` — the agents already on *this* branch, innermost last.

A human post mints `{ root: self, spent: 0, path: [] }`. An agent turn triggered
with a relay posts its reply carrying `{ root, spent + 1, path + [self] }`.

This has to ride on the wire. The device that fires the trigger and the device
that runs the mentioned agent are routinely different machines, so the chain
cannot live in one daemon's memory the way §1.1 engagements can. That means a
defaulted `relay: Option<Relay>` on `ChatMessage` — absent from an older peer's
message, which reads as "no allowance", the current behaviour.

### 3.3 Termination

Three bounds, because each one alone has a shape it does not catch:

- **Budget.** `spent < max_relay_turns` (default **20**, hard ceiling 50). This
  is the only bound that holds regardless of the cascade's shape — depth limits
  alone do nothing about a wide fan-out, and fan-out limits alone do nothing
  about a long chain. It is a whole-cascade counter, not a per-branch one.
- **Fan-out.** At most **one** agent-level mention in an agent reply is honoured
  — the first that is not the agent itself. An agent that names three agents
  gets one trigger and two chips. Without this, a budget of N is a budget of N
  *levels*, i.e. exponential rather than linear.
- **No self-trigger.** An agent never wakes itself. Note what "itself" means:
  the same agent *on the same machine*. Several devices may each configure an
  agent called `claude`, and one handing off to another is delegation, not a
  loop — an early cut compared names alone and silently swallowed exactly that
  hand-off. The check is made where it can be exact, on the receiving device,
  by comparing peer ids; the sending side only skips a mention it can already
  tell is local (bare, or scoped to itself), which also frees the one honoured
  hand-off for the next mention. This kills the degenerate one-agent loop and
  is the only cycle worth forbidding structurally.

An earlier draft added **path acyclicity** — an agent already on the branch is
never re-triggered — which makes `A → B → A` impossible rather than merely
budgeted. It was dropped, and the budget raised from 3 to 20 in exchange. The
reason is that acyclicity forbids exactly the thing delegation is *for*: two
agents iterating on a problem. With acyclicity in force, a budget above 3 is
close to meaningless, because spending it requires a chain of that many
*distinct* agents. Ping-pong is therefore allowed and bounded by arithmetic:
20 turns is roughly ten exchanges between two agents, which is a real working
session and still a bill a person can absorb if they walk away from the
keyboard. The ceiling of 50 exists so a mistyped config cannot hand one chat
message an unbounded bill.

A human message always re-mints a full budget — including a human message that
arrives mid-cascade. Humans are not rate-limited by their agents' spending.

Note the asymmetry with §1.1: a relayed turn does **not** open an engagement
window for the mentioning agent. Follow-up routing is a convenience for humans
typing quickly; giving it to agents would hand them an unbudgeted second turn
through a side door.

### 3.4 Enforcement is local, as always

`spent` and `path` arrive over the wire from a peer that could have forged them.
That is not a new exposure — a hostile peer can already post `@claude` in a loop
— but it does mean the wire value cannot be the enforcement point. The receiving
device:

1. clamps to its own configured maximum: `effective = min(wire.spent_remaining,
   local_max)`;
2. keeps its own per-`root` counter, so a cascade that re-enters the same device
   several times cannot spend more than that device allows in total;
3. checks the agent allowlist and `reaction = "push"` exactly as it does for a
   human mention.

The wire field is a hint that shrinks; the device's own count is the truth. This
keeps the per-device execution gate invariant intact — nothing here moves a
run decision onto the wire.

### 3.5 Opt-in, on the receiving side

**Superseded.** This section described a per-agent `accept_from` switch, so that
a hand-off only landed on an agent that had opted into receiving one. That
shipped, and was then removed.

The argument for it was that the device spending the tokens should decide. That
argument is sound and still holds — but the device already decides, twice, before
this switch is ever consulted: the agent has to be in `agents.toml` at all, and
the reaction policy has to be `push`. A third switch added no authority, and it
failed in the worst available way: a hand-off to an agent that had not opted in
did nothing, said nothing, and looked exactly like a bug. It was reported as one.

So delegation is no longer opt-in. An agent allowed into a Circle is reachable by
the other agents in it. What remains configurable is how far a chain may run
(`max_relay_turns`), not who may start one — a bound rather than a gate, which is
what §3.3 argued for in the first place.

Settings are scoped instead of per-agent, because whether an agent should read
the room or how long a chain may run is a property of *where* it is working:

```toml
# Global — applies in every Circle.
reaction = "push"
engagement_window_secs = 180
ambient = ["claude"]
max_relay_turns = 20

# One Circle, overriding only what it names.
[circles."<circle-id>"]
ambient = []
max_relay_turns = 4
```

Still device-local, and that part is not negotiable: a synced per-Circle setting
would let a remote member decide what this machine spends.

### 3.6 What a relayed turn may do

A relayed turn is not an unaddressed turn — someone asked for it, just not
directly. So it does not inherit §2.4's `pending` treatment: writes land and are
accepted, as they do for any mention, and revertibility comes from the proposal
store as usual.

What changes is attribution. `TriggerOrigin` resolves from the **root human**,
not from the mentioning agent — a cascade rooted in a local user's message is
`LocalUser` throughout, one rooted in a remote member's is `RemoteMember`. An
agent must not be able to launder a remote member's request into a local one by
relaying it. The proposal record additionally stores the branch as
`relay_path` — the agents that relayed the work, *excluding* the one that wrote
the files, which is already `actor_id`. A proposal reading
`actor_id: "codex", relay_path: ["claude"]` says a person asked claude and
claude asked codex. Without it a delegated change is indistinguishable from one
the user asked for.

### 3.7 Legibility

A cascade that is invisible is indistinguishable from a runaway. Three things
are the minimum:

- Relayed messages render their provenance — `codex · via @claude` — so a user
  can see that a turn they did not ask for was asked for on their behalf.
- Exhausting the budget posts nothing to chat (a `system` post per dead mention
  is exactly the transcript noise §1.3 is trying to remove) but *is* surfaced in
  the activity indicator as a `skipped` activity carrying its reason:
  `codex not triggered · relay budget spent`. A stopped cascade reports itself
  the same way. An agent that simply has not opted in (`accept_from`) stays
  silent — that is this device's configuration, not an event in the room, and
  announcing it on every mention would leak local config as chatter.
- A cascade is stoppable: `POST /api/chat/relay/stop` with the cascade's
  `root`, surfaced as a **stop chain** button that appears while a relayed turn
  is running. Anyone in the Circle may stop one — the person watching is not
  always the person who started it, and a wrongful stop costs a re-mention.

  Note what this is *not*. It does not interrupt the turn in flight, because
  enoxian has no control that cancels a running agent; it stops every
  **further** turn, which is what actually bounds the spend. The draft here
  assumed a per-agent cancel existed to hang this on. It does not, and building
  one is its own piece of work.

  The stop is written into the **synced control doc**, not held in one daemon's
  memory: the device that would run the next turn is usually not the device
  whose user hit stop, so a local flag would stop nothing.

The durable inbox now retains stop markers across restart without a time-based
expiry and rechecks them before launching queued work. If the control document is
busy, the worker defers the request and retries; it does not misreport cancellation
or assume permission to launch.

Historically, reading the stop list needed a
transaction on the control doc, which is routinely busy, and the first cut
treated "cannot read" as "stopped". That is the instinctive choice for a brake
and it was wrong: every busy moment silently refused a delegation, so the
feature broke at random. The legacy boolean helper fails **open** with a warning; inbox execution uses the
retryable check described above. The asymmetry
justifies it — spend is bounded by the budget and the per-root ledger, neither
of which touches this doc, so a missed stop costs one extra turn, while a false
stop costs the whole feature.

### 3.8 Interaction with ambient

Ambient turns (§2) are still triggered by human messages only. A relayed reply
is agent-authored, so it does not wake ambient listeners — otherwise one
delegation would broadcast to every ambient agent on every device, and the
budget would be spent by agents that were never addressed.

The two features compose in one direction only: an ambient turn may *mint* a
relay chain from the human message it is responding to, spending from that
message's budget. An ambient agent that passes (§2.3) spends nothing.

## 4. Phasing

The original rollout put delegation first. The working-tree implementation now
replaces the Circle-wide execution lock with the parallel runtime described above.
The historical sequencing below explains the dependencies.

1. **Run queue** (§1.3) — independently useful, and a prerequisite for the
   rest.
2. **Engagement window + composer affordance** (§1.1, §1.2) — the bulk of
   the UX win.
3. **`author` field** (§2.1) — small, defaulted, unlocks §2 and is useful on
   its own for rendering.
4. **Ambient engagement behind per-agent opt-in** (§2) — with the heuristic
   gate from day one, and §2.4 settled before any of it landed.
5. **Agent-to-agent delegation** (§3) — the relay chain, the three bounds, and
   `accept_from`.
6. **Reply-threading** (§1.4) — the explicit form, for when the window guesses
   wrong.

## 5. Explicitly not doing

- **Ambient as a default or a Circle-wide setting.** Per-device opt-in only.
- **Removing the mention gate.** Mentions remain the explicit, unambiguous way
  to address an agent; everything here is additive.
- **Unbounded agent-to-agent conversation.** §3 allows delegation under a
  budget that a human mints and agents only spend. What stays foreclosed is a
  cascade that can sustain itself: no agent-minted budgets, no agent-opened
  engagement windows (§3.3), no ambient turns woken by agent output (§3.8).
- **Coalescing queued messages** into a single turn (§1.3).

## 6. Docs updated when this shipped

- [examples/agents.toml](../examples/agents.toml) — `engagement`,
  `accept_from` and `max_relay_turns` keys; the header comment about reacting
  only to `@mentions` becomes wrong.
- [guide/agents.md](../guide/agents.md) — follow-up routing, the flow
  diagram's new entry paths, `engagement`, the PASS convention, and delegation
  (`accept_from`, the relay budget, `via @agent` rendering).
- [reference/api.md](../reference/api.md) — chat schema gains `author` and
  `relay` (later `reply_to`); event table may need an implicit-routing sibling.
- [concepts/proposals.md](../concepts/proposals.md) — only if §2.4 lands;
  ambient would be `pending`'s first real use as a gate.
- [concepts/security.md](../concepts/security.md) — the §2.6 privacy posture,
  and why a forged `relay` on the wire is not an escalation (§3.4).
- [concepts/internals.md](../concepts/internals.md) — engagement store, run
  queue, and per-root relay counters under Agent Runtime.
- `CHANGELOG.md` and [index.md](../index.md).
