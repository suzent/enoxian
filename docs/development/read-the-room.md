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
2. **`ambient_backlog_tail` default.** 1 is conservative. A Circle used across
   time zones might want the last handful of a backlog read — but probably as
   *context* on one turn rather than as several turns.
3. **Should a drained burst be concatenated?** The tail message carries the turn
   and the others reach the prompt only through the normal history window. That
   is probably enough; if not, the drain could mark them as one thought.
4. **Per-agent turn timeouts.** `ambient_turn_timeout_secs` is device-wide,
   because it bounds a shared device resource. A slow local model and a hosted
   one are not comparable, so a per-agent override may be worth it.

## 2. Held Draft — the half that is missing

From [Raft's AX post][raft], discussed in the `enox-dev` Circle. Before an agent
sends, re-check the room; if it moved, hand the message **back to the agent** to
rewrite, force-send, or stay silent.

What exists is an *ingress* check: a queued unaddressed turn is cancelled if
another agent answered while it waited. That is not this mechanism. It fires
before inference, and the harness decides — the agent never learns the room
moved. The design this is drawn from is explicit that a system which drops a
message without handing it back is the version to oppose.

The egress slot is still empty. An agent's reply is posted with no re-check
between inference finishing and the post, which is thirty seconds to three
minutes of unguarded staleness on every unaddressed turn.

Constraints worth keeping if it is built:

- **Force-send must exist.** Without an escape hatch, a check becomes censorship.
- **The agent should declare its own precondition** ("hold me if someone answers
  the DB question") rather than the harness hardcoding what counts as a conflict.
- **Bound the retries**, or a busy room livelocks on rewrite → hold → rewrite.
- Instrument the choice distribution. If nearly every hold ends in force-send,
  the mechanism is pure overhead and should be removed.

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
