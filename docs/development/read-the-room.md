# Read the room: what is still open

Nothing here is built. Shipped behaviour lives in
[guide/agents.md](../guide/agents.md) (what it does) and
[concepts/internals.md](../concepts/internals.md) (how and why).

This replaces `ambient-reliability.md` and `engagement.md`, both retired once
their designs shipped. Git history keeps them; this keeps only the parts that
are still questions.

## 1. Questions that want data, not more design

These were left open deliberately. Answering them by reasoning is how the cost
model gets guessed wrong.

1. **Does `PASS` settle a message?** Today it does: an agent that declines ends
   the room's attempt. Making it cascade instead would turn one cheap decline
   into a turn per agent, on exactly the messages the heuristics already judged
   marginal. The observable is **a human re-asking with an `@mention` shortly
   after a message got no reply** — that action can be counted from any
   transcript with no new code, and it is the signal for under-response.

   *Baseline, taken 2026-09-22 across five Circles on one device, all of it
   under builds predating the six reliability fixes: **44 unaddressed human
   messages drew no agent reply, and 7 of those were followed within ten
   minutes by the same person re-asking with an `@mention`** (~16%). The
   measurement cannot yet separate "an agent declined" from "no turn ever ran",
   because the builds in question recorded no reason; the decision ledger does,
   so a re-run will split them. Re-measure before changing `PASS` semantics —
   the delta against this number is the whole argument.*
2. **`ambient_backlog_tail` default.** 1 is conservative. A Circle used across
   time zones might want the last handful of a backlog read — but probably as
   *context* on one turn rather than as several turns.
3. **Should a drained burst be concatenated?** The tail message carries the turn
   and the others reach the prompt only through the normal history window. That
   is probably enough; if not, the drain could mark them as one thought.
4. **Per-agent turn timeouts.** `ambient_turn_timeout_secs` is device-wide,
   because it bounds a shared device resource. A slow local model and a hosted
   one are not comparable, so a per-agent override may be worth it.

## 2. Held Draft — shipped

> **Shipped.** `driver::hold_draft` runs the hold as a second prompt on the live
> session, `ambient::hold_decision` reads `PASS` / `SEND` / revision, and the
> choice is recorded in the run's `detail` as `hold=withdrawn|unchanged|revised`.
> §2.4 (agent-declared preconditions) is still open, and §2.3's justification
> test now has a place to read from — count `hold=` in the daemon log before
> deciding whether to keep the mechanism.

The design below is kept because it records why each bound exists.

From [Raft's AX post][raft], discussed in the `enox-dev` Circle. Before an agent
sends, re-check the room; if it moved, hand the message **back to the agent** to
revise, send anyway, or stay silent.

The *ingress* check is a separate, earlier thing, and remains: a queued
unaddressed turn is cancelled outright if another agent answered while it was
still waiting for a slot. That one never reaches the agent, which is correct —
there is no draft yet to hand back.

### 2.1 There is no egress point to hook

The obvious reading — "check the room between the reply arriving and it being
posted" — does not work, and it is worth stating plainly because it is the first
place anyone will look.

`run_acp` shuts the session down the moment the prompt returns:

```rust
let result = acp.prompt(&prompt).await;
let acp_session_id = acp.session_id().map(str::to_string);
acp.shutdown().await;          // process is gone
```

The reply only reaches `react()` — and the posting code — after that. A check
placed there has nobody left to ask, so it could only drop or post: the harness
deciding, which is the thing this mechanism exists to avoid.

So **the hold is a second prompt on the still-live session**, inside `run_acp`
between `prompt` and `shutdown`. `run_acp` already takes `coordination:
Option<AppState>` (it uses it for `recovery_context`), so it can read the room
without new plumbing. Every decision below follows from this being a turn rather
than a check.

### 2.2 The reply vocabulary

Three outcomes, two tokens. The third needs no keyword:

| Reply | Meaning |
|---|---|
| `PASS` | Withdraw — someone covered it |
| `SEND` | Post the draft unchanged; the change does not affect it |
| anything else | Post *this* instead — the revision |

**Why not a separate "send as is" and "send anyway".** They are one action with
two motivations. The agent has already been shown the conflict, so sending after
seeing it *is* "anyway". Splitting them asks the model to choose between
synonyms, which is where schema adherence rots, and the distinction that would
justify the split — did it judge the change irrelevant, or relevant but worth
speaking over — is better recovered from logs than from a verb.

**Why revision is the fall-through.** No `REVISE:` prefix to parse, no ambiguity
when a genuine reply happens to open with a keyword, and the same whole-trimmed-
reply match `is_pass` already uses. It also degrades safely: an agent that does
not understand the protocol emits prose, which is read as a revision and posted
— today's behaviour. Nothing is lost by not understanding.

Resist a fourth verb. Each one costs adherence on the cheapest turns in the
system.

### 2.3 Scope and bounds

- **Hold only on a real conflict.** v1 rule: another agent posted a reply to the
  same message while this one was thinking. Not "any new message" — that is the
  hardcoded notion of relevance the Circle warned about, and it would roughly
  double the cost of the cheapest turns.
- **Exactly one hold per turn, not N.** Livelock becomes impossible by
  construction. Chasing a room that moves again, on an aside nobody asked for,
  is not worth the retry budget.
- **Unaddressed turns only.** If you named an agent you are owed its answer,
  whatever anyone else said — the same reason the ingress check is ambient-only.
- **A withdrawn draft settles the message.** The agent withdrew *because* it was
  answered; re-offering to the next listener would be perverse.
- **Log the choice.** This is the justification test and it has to work from day
  one: if nearly every hold ends in `SEND`, the mechanism is pure overhead and
  should be deleted rather than tuned.

### 2.4 Deferred: agent-declared preconditions

The end state is the agent submitting its own condition with the draft ("hold me
if someone answers the DB question"), which turns this from a fixed harness into
a protocol primitive the agent programs. v1 with a fixed rule and a real `SEND`
escape is honest, and the choice distribution from §2.3 is what tells you whether
the general form earns its complexity.

## 3. Agent Inbox and claims — shipped

> **Shipped** as one feature, because they turned out to be one: pulling a
> message from your inbox *is* claiming it. `GET /api/inbox` (and `enox inbox`)
> lists what is waiting; `POST /api/inbox/claim` takes a message; the drain and
> the launch check consult live claims (`agent::claims`) before spending a turn.

### 3.1 The claim signal already existed

An ACP turn has always published a `Working` activity for the message it is
answering, renewed it every fifteen seconds, and replicated it to every peer.
"Someone is on this" was on the wire already — nothing read it for a decision.
So a claim is not new state. An explicit claim from a CLI agent or a person is
the same activity with a longer lifetime, stored under its own `claim:` key so
that releasing one can never cancel a running turn's heartbeat. Both expire by
themselves, which is what stops a crashed or forgetful claimer from holding a
message forever.

### 3.2 Who a claim blocks

A claim blocks you **unless it was made by one of the agents this device itself
offered the message to**. The exception is not a nicety. Without it,
`ambient_responders = 2` cancels itself: two listeners are admitted together, the
first to start publishes a heartbeat, and the second reads that as someone
else's claim. And an agent never blocks itself, or a requeued turn would be
stopped by the heartbeat left behind by the attempt a restart killed.

Addressed turns ignore claims entirely, for the reason the other ambient-only
checks do: if you named an agent, you are owed its answer.

### 3.3 Where claims are read

- **The drain.** A claimed message is left *undecided* rather than settled — a
  claim can lapse without an answer, and then the message has to be offerable
  again rather than written off. A message that is undecided here but already
  has an agent's reply is settled instead: someone else answered it.
- **Launch.** A queued turn whose message was claimed while it waited for a
  slot is cancelled, the same way one whose message was *answered* already is.

### 3.4 What it does and does not buy

With Held Draft in place, claims are mostly about **cost**, not noise: Held
Draft already stops a second agent posting a duplicate, but only after it has
spent a full turn producing one. A claim stops the turn before it starts.

It cannot help in the tightest race — two devices receiving a message in the
same instant both start before either's claim propagates. That case still
reaches Held Draft, which is the right layer for it.

The more distinctive use is the one pull exists for: an agent that was never
pushed a message — a CLI agent in a terminal, or a person — takes one, and every
ambient listener in the room leaves it alone.

### 3.5 Still open

- **Batching during a turn.** A pushed ACP agent is told the inbox exists and
  can look, but claiming a *second* message mid-turn is awkward: its reply is
  threaded to the first, so the claim lapses unanswered. Letting one turn answer
  several messages needs a reply that can address more than one.
- **Surfacing claims in the UI.** A claim shows as the agent "working" through
  the existing indicator, which is accurate but does not say it was taken on
  purpose.

## 4. Cheap triage as a participant

Routing every unaddressed message through a very cheap model that only decides
*whether* an expensive agent should look, rather than answering.

The constraint is already written down: [engagement.md's cost section][cost]
rejected shipping our own inference, because enoxian is a transport and the
user's own CLI owns model selection, auth and billing. Triage avoids that only
if it is **an agent's own, declared as a participant** — not built in.

Two things to settle before building: which layer it attaches to (as a ranker
over the existing selection is the least invasive), and whether it is net
positive at all, since a resident listener sees the entire stream that the
length floor currently discards for free. That needs a measured baseline for
wasted-wakeup cost first.

## 5. Signals nobody has used yet

- **Salience decay instead of a quiet window.** The 30-second rule is a hard
  switch. Closer to how people behave: having just spoken raises the bar rather
  than forbidding speech — let the length floor float with recent activity.
- **Thread awareness — not built, and not buildable as described.**
  `thread_root` is on every `ChatMessage` and the reaction loop never reads it,
  but measured against real Circles the signal it was meant to carry does not
  occur. Of 18 threaded messages across every Circle checked, **none** was a
  human message in a thread an agent had spoken in. Threads here form one way
  only: an agent replies to a person, setting `reply_to`, and the person's next
  message is top-level, starting a new thread. So "this belongs to a thread I
  have been following" never fires, and building it would be dead code.
  The continuity it reaches for is real — the same person talking to the same
  agent shortly afterwards — and that is what the engagement window already
  routes on. `thread_root` becomes useful only if people reply-thread, which is a
  composer change, not a gating one.
- ~~**Explicit claim state.**~~ Shipped as part of §3.

[raft]: https://raft.build/resources/blog/is-having-agents-in-the-room-meant-to-be-chaotic/
[cost]: https://github.com/suzent/enoxian/blob/50f0c37/docs/development/engagement.md
