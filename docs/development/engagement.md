# Agent engagement

How an agent in a Circle decides that a chat message is *for it*, and what it is
allowed to do once it decides. Forward-looking: only mention routing exists
today.

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
- An agent's reply never wakes another agent (`fire_mentions = false`).
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

- **Timeout.** Default 3 minutes since the agent's last reply, renewed by each
  reply. Configurable; `0` disables the feature.
- **Redirection.** The speaker mentions any agent — including the same one.
  Explicit addressing always wins and re-arms the window.
- **Exit.** The speaker dismisses it in the composer (below).

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

Concurrent runs are already prevented, but by a lock that is both too coarse
and too blunt for conversational use. `driver::launch` refuses to start when any
managed change session is open:

> managed agent '…' is already running in this Circle (session …)

That is **Circle-wide**, not per agent: while `@claude` works, mentioning
`@codex` fails too. And it fails *loudly* — `react()` turns the error into a
`system` chat post. Under mention-only usage that is tolerable, because a second
mention during a run is rare and the message explains itself. Follow-up routing
makes rapid consecutive messages the normal case, and the same lock turns every
one of them into a failure notice in the transcript.

So the work here is **narrowing the lock and adding a queue**, not adding
mutual exclusion that is missing:

- Scope the guard to `(circle, agent)` so unrelated agents run in parallel.
  This is the part that needs care: `LocalChangeSession::load_managed` is
  currently a single per-Circle record, and the proposal baseline story assumes
  one managed writer at a time. Two agents writing concurrently against one
  baseline is a genuinely open question — **if it does not hold, keep the
  Circle-wide lock and queue across it**, which still fixes the UX.
- Queue subsequent messages for a busy agent in arrival order and deliver them
  as separate turns, rather than failing them into chat. Cap the depth (say 4),
  dropping the oldest with a visible note.

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

Goal: with several agents in a Circle, let them *see* the conversation and
volunteer, instead of being summoned one at a time.

This is the more interesting idea and the more expensive one. The design below
is deliberately conservative about what an unaddressed agent may do.

### 2.1 Trigger rule

An ambient turn is offered only for **human-authored** messages. Agent replies
and system messages never trigger one.

This is what keeps the system from oscillating: agent output cannot become
another agent's input, so there is no fixpoint to chase and no token spiral. It
is the same property `fire_mentions = false` buys for mentions today, stated as
a rule about authorship rather than a flag on one call site.

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

If that proves too invasive, the fallback is narrower and still safe: an
ambient turn that wants to change files says so in chat, and a person mentions
the agent to actually do the work.

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

Recommendation: ship the heuristic debounce with the feature, measure real
Circles, and only then decide whether anything smarter is warranted.

### 2.6 Privacy posture

Ambient engagement changes what leaves the Circle. Under mentions, casual chat
reaches a model provider only when someone deliberately summons an agent. Under
ambient, **every human line is sent to every configured provider on every
device that opted in** — possibly several vendors simultaneously, for the same
sentence.

That is a defensible trade for a working Circle and an unpleasant surprise for
a social one. It needs to be stated plainly at the point of opt-in, and the
roster must mark ambient agents so *all* peers can see who is listening — not
just the device that configured one. This is a trust-model change and belongs
in [concepts/security.md](../concepts/security.md) when it ships.

## 3. Phasing

1. **Run queue** (§1.3) — independently useful, and a prerequisite for the
   rest.
2. **Engagement window + composer affordance** (§1.1, §1.2) — the bulk of
   the UX win, backend-shaped, no schema change.
3. **`author` field** (§2.1) — small, defaulted, unlocks §2 and is useful on
   its own for rendering.
4. **Ambient engagement behind per-agent opt-in** (§2) — with the heuristic
   debounce from day one, and §2.4 settled before any of it lands.
5. **Reply-threading** (§1.4) — when the window's guesses prove annoying in
   practice.

## 4. Explicitly not doing

- **Ambient as a default or a Circle-wide setting.** Per-device opt-in only.
- **Removing the mention gate.** Mentions remain the explicit, unambiguous way
  to address an agent; everything here is additive.
- **Agent-to-agent conversation.** The human-authorship trigger forecloses it
  on purpose. Multi-agent collaboration is a real design question, but it needs
  a termination story that this spec does not have.
- **Coalescing queued messages** into a single turn (§1.3).

## 5. Docs to update when this ships

- [examples/agents.toml](../examples/agents.toml) — `engagement` key; the
  header comment about reacting only to `@mentions` becomes wrong.
- [guide/agents.md](../guide/agents.md) — follow-up routing, the flow
  diagram's new entry paths, `engagement`, and the PASS convention.
- [reference/api.md](../reference/api.md) — chat schema gains `author` (later
  `reply_to`); event table may need an implicit-routing sibling.
- [concepts/proposals.md](../concepts/proposals.md) — only if §2.4 lands;
  ambient would be `pending`'s first real use as a gate.
- [concepts/security.md](../concepts/security.md) — the §2.6 privacy posture.
- [concepts/internals.md](../concepts/internals.md) — engagement store and run
  queue under Agent Runtime.
- `CHANGELOG.md` and [index.md](../index.md).
