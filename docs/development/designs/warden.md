# The warden

How the warden decides who answers an unaddressed message, what an experiment
may change about it, and what is still open. For how to use it, see
[the warden](../../guide/agents.md#the-warden).

## Where it sits

```
new message (a person's, or an agent's with agent_messages on)
 ├─ names an agent   → dispatched to that agent; the warden never sees it
 └─ names no one     → its cascade has budget left (agents' messages)
                     → not part of a person's exchange with an agent, not
                       already answered or claimed
                     → soft gates: length floor, quiet window   (spec.gates)
                     → warden: decide                            (spec + decider)
                     → offer: nobody | single | room | vote, never to the author
```

Every new message is one decision. A posted answer is itself the newest
message: if it is wrong, incomplete, or asks something, the warden decides on
it like any other, so there is no separate review path. A message that has
already been answered is left alone because its answer is now the message to
decide about.

The code is in `src/agent/warden/`:

| File | Role |
|---|---|
| `spec.rs` | The policy, as data. Validated whole, replaced whole. |
| `decider.rs` | Manifests, the two HTTP protocols, keys. Answers questions with probabilities. |
| `policy.rs` | Which questions to ask; how answers become a choice; the rules. |
| `mod.rs` | Per-Circle runtime: waiting for a decider without blocking the drain. |
| `vote.rs` | Blind vote: hold ballots, compare conclusions, post one, or add a voter. |
| `log.rs` | `warden.jsonl`: every decision, what ran, every vote round. |
| `episodes.rs` | The log joined with the execution inbox and run outcomes. |

The drain (`reaction::drain_ambient`) is synchronous. With the rules, a
decision is made inline. With an HTTP decider, the first drain that reaches a
message starts the request and leaves the message undecided; when the answer
arrives (or the timeout passes and the rules decide), `Runtime::ready` wakes
the reaction loop, and the next drain carries the decision out. A message is
recorded in the scry ledger only once it has been decided, so a restart while
waiting loses nothing.

## Decisions

The decider is asked, about one message and the six lines before it:

| Question | Type | Used for |
|---|---|---|
| `admit` | yes/no | Below `admit_threshold`, nobody answers. |
| `agent` | choice over listeners, each described by its [card](../../guide/agents.md#agent-cards) | Order of listeners. |
| `success:<mode>` | yes/no per mode that can run | `p − λ × cost(mode)`, best wins. |

A refused or missing answer falls back to the rules for that part only. A mode
is asked about only when it can run: a vote or a room needs two listeners and a
Circle ceiling (`ambient_responders`) of at least two.

An agent's message is asked about in its own words (`questions.admit_agent`):
it needs a reply when it is wrong or incomplete, asks something, or leaves a
step for someone, not merely when it is a question. Only a decider can answer
that. The rules, and a decider that refuses `admit` or does not answer in time,
leave an agent's message be: admitting it on length would have agents answer
each other until the budget ran out.

The vote's agreement check is the one other point a decider is asked: with
`spec.vote.ask_decider` set, each answer is compared to the first of every
existing group (`agree`, yes/no). Otherwise answers agree when their `ANSWER:`
lines match after normalization.

**Calibration.** A general decision model does not know these agents, so the
spec can map each question's raw probability onto how often it turned out
right: `[calibration] admit = [[raw, calibrated], …]`, raw ascending, linear
between points. Keys are `admit`, `admit:agent` (an agent's message),
`agent:<name>` and `success:<mode>`. Every decision records the calibrated probabilities it acted on,
and the decider's raw ones under `raw`, so a harness can fit the next
calibration from episodes and their outcomes, and propose it as part of a spec.

## What an experiment may change

The warden is meant to be improved by experiments, including automated ones
(RSI: propose a change, test it, keep what wins). What they may touch is fixed
in three layers.

**Never** — in code, not in the spec, and not adjustable by anything an
experiment produces:

- Every agent turn woken by an agent's message, through a mention or the
  warden, is charged to the person's message at the root of its cascade, and
  stops at `max_relay_turns` (at most 50). This, not who wrote a message, is
  what ends a conversation agents carry on among themselves.
- Whether agents' messages reach the warden at all (`[warden]
  agent_messages`) is the device's configuration, not the spec's: it widens
  what can wake an agent.
- A call by name bypasses the warden, and the device's concurrency holds.
- Decisions, keys, the log and episodes stay on the device.
- How an experiment is scored: the outcome signals, the dev/test split, the
  acceptance gate. An experiment that could change its own score would
  optimize the score.
- Review: a spec is applied by a person (`enox warden spec apply`, or the
  panel), never by the experiment that proposed it.

**By a person, in code** — an experiment can only bring evidence:

- The prompts' framing (`context.rs`, `ambient.rs`, `warden::instruction`).
- The set of actions (nobody, single, room, vote) and the set of questions.
- Internal constants: how much history a prompt carries, debounce, requeue
  limits.
- The outcome detectors (`outcome.rs`).

**By an experiment, as data** — proposed as a new spec, with evidence:

| What | Where | How it is tested |
|---|---|---|
| The spec: allowed modes, `admit_threshold`, `lambda`, costs, vote sizes, gates, rules, question wording | `~/.enoxian/warden/spec.toml` | Offline replay against episodes and benchmark results |
| Scry parameters: listeners, rotation, backlog tail, attempts | `agents.toml` | Online only: they change what runs |
| Lessons and collaboration conventions | The Circle's `AGENTS.md` | Online only |
| Agent cards | `agents/` in the shared folder | With the spec, offline |

Changes to the spec only change *which* existing way of answering is chosen,
so they can be replayed against a full table of results; that is where an
experiment should start. Lessons and scry parameters change what agents do,
so old results no longer apply and every candidate has to run again.

## Episodes

`enox warden episodes` (or `GET /circles/<id>/api/warden/episodes?format=jsonl`)
prints one object per message the warden has a record of:

```json
{
  "message_id": "…",
  "features": { "text": "…", "weighted_len": 812, "context": ["…"],
                "candidates": [{ "name": "claude", "summary": "…" }], "ceiling": 3 },
  "judgements": [{ "phase": "act", "decision": { "choice": { "mode": "vote", … },
                   "p_admit": 0.93, "p_success": { "single": 0.55, "vote": 0.78 }, … },
                   "executed": { "by": "warden", "agents": ["claude", "codex"] } }],
  "votes": [{ "groups": [["claude", "codex"]], "action": "posted", "posted_by": "claude", … }],
  "runs": [{ "agent": "claude", "status": "completed", "detail": "Voted",
             "usage": { "cost": { "amount": 0.031, "currency": "USD" }, … },
             "outcome": { "proposals": [], "called_after_pass": null } }],
  "answered": true,
  "spend": { "USD": 0.058 }
}
```

`features` is everything the decider saw, so a different spec or decider can
be replayed on it without the room. The outcome fields are the external
signals an experiment scores by: a pass followed by someone calling an agent
by name (`called_after_pass`), proposals accepted or reverted, and spend.
Correctness on a benchmark comes from the benchmark's grader, joined by
message id.

A typical loop:

1. Run a Circle in `shadow` (or `act` with the current spec) and collect
   episodes, or run a benchmark through it.
2. Offline, replay candidate specs against the episodes and the benchmark's
   full result table (every mode, every question). Keep the ones no other
   beats on both accuracy and cost.
3. Draft the best as a spec file, with the evidence beside it.
4. A person reviews it and applies it. The version is recorded on every
   decision from then on, so its effect can be measured.

### From an offline policy to a spec

An offline harness scores policies in its own terms. Its result becomes a spec
like this:

| Offline policy | Spec |
|---|---|
| Two length buckets, short → A, long → B | `rules.long_chars`, `rules.short_mode`, `rules.long_mode`, with `builtin:rules` as the decider |
| Allowed actions | `modes`: `single`; `vote` for vote-adaptive (two samples, a third on disagreement); `room` with `room_size = 3` for three listeners together |
| Vote-adaptive's sizes | `vote.start = 2`, `vote.max = 3` |
| Cost per action, in tokens | `costs.*` as multiples of a single answer's tokens: `vote = tokens(vote) / tokens(single)` |
| λ per thousand tokens | `lambda = λ × (single's thousand tokens)`, since costs are in single-answer units |
| Admission threshold | `admit_threshold` (only with a decider that answers `admit`) |

Set `version` to something the harness can find again (`t1-ppc-0007`); every
decision made under the spec records it.

### Running a benchmark through the warden

1. On the device that runs the listeners: `enox warden mode act`, and set
   **Listeners per message** to at least the largest vote (`vote.max`).
2. `enox warden spec apply <spec.toml>`.
3. Post each question to the Circle without naming an agent. The warden's
   choice, the turns it ran and the vote are recorded against the question's
   message id.
4. The posted answer is the agent reply to that message. A vote's reply ends
   with an italic *Blind vote: …* line after the voter's own `ANSWER:` line;
   graders should read the `ANSWER:` line.
5. `enox warden episodes --since <start>` and join with the grader's results
   by message id.

## Open questions

- **Fitting the calibration.** The spec can hold a calibration, and every
  decision keeps the decider's raw figures beside the calibrated ones, but
  nothing fits it yet. That belongs to the harness, from a few hundred
  episodes with outcomes.
- **Ballots across restarts.** Ballots are held in memory. A restart mid-vote
  loses them, and the message is left unanswered rather than answered twice.
- **Lessons.** Choosing which lessons from `AGENTS.md` to put in front of an
  agent, by message type, is not done: the brief points at the whole file.
