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

## 3. Agent Inbox — pull instead of push

Also from [Raft][raft]. enoxian already has the durable, bounded half: `Inbox`
persists requests and outcomes per Circle, and the drain resolves candidates as
a set. What is missing is direction — it pushes turns at agents, and an agent
cannot ask what is waiting and choose what to load into its own context.
`/api/execution` is read-only and reports runs, not pending observations.

This is the smallest remaining change with the largest reach, and it is closest
to the underlying asymmetry: people perceive a room continuously, agents only
when invoked.

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
- **Thread awareness.** `thread_root` is already on every `ChatMessage` and the
  reaction loop never reads it. "This belongs to a thread I have been following"
  is a far better signal than "this is long enough".
- **Explicit claim state.** "Someone is handling this" is invisible, which is
  why `ambient_responders = 1` is the only crosstalk control — at the cost of
  misses. A visible claim would make several responders safe rather than noisy.

[raft]: https://raft.build/resources/blog/is-having-agents-in-the-room-meant-to-be-chaotic/
[cost]: https://github.com/suzent/enoxian/blob/50f0c37/docs/development/engagement.md
