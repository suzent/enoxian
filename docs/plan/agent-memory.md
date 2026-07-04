# Agent Memory And Recovery — Design

**Status:** Model settled; core behavior implemented. The "chat inbox" recovery
feature is a future task.

How a mentioned agent remembers context across mentions, and how it recovers
when it loses that memory.

---

## Model: stateful by default, chat-recoverable

The agent is **stateful**. Each agent has a persistent ACP session per
`(circle, agent)`; the session id is stored on disk
(`~/.enoxian/circles/<id>/agent_sessions/<agent>.session`) and resumed on the
next mention via `session/load`, so the agent continues from its prior context.
This is the fast path — the agent's own working memory.

But an agent can **lose its session** (the ACP adapter forgets it across its own
process restarts; the session file is cleared; the agent is new to the circle).
The design does not treat that as data loss, because there is a **durable record
the agent can recover from: the circle chat**. On a fresh (non-resumed) session,
enoxian hands the agent the standing brief plus recent chat so it can catch up
from the shared history.

So there are two history surfaces, related as **primary + recovery**, not as
competing duplicates:

| Surface | Role | Durability |
|---------|------|-----------|
| ACP session | the agent's working memory (fast) | can be lost |
| Circle chat | the durable shared record it recovers from | must persist (see below) |

This is why chat persistence matters here, not just for humans: the chat is the
agent's fallback memory. It is tracked as [control-persistence.md](control-persistence.md)
(M14.5) — without it, an agent that loses its session on an all-offline restart
has nothing to recover from.

---

## What this means for context injection (implemented)

`src/agent/context.rs::build_prompt` already follows the model:

- **Resumed session** — the agent holds the brief and prior turns in its own
  memory. Send only a lean per-turn cue (`{sender} mentioned you … continue from
  your prior context`). **Do not** re-inject the chat transcript — duplicating
  what the agent already has produced the "greeting soup" bug (multiple messages
  run together) and wastes context.
- **Fresh session (also the recovery path)** — inject the full standing brief +
  recent chat so the agent catches up from the durable record, then the task.

Combined with the reply-segmentation fix (post the agent's *final* message, not
a concatenation of every streamed message), replies are clean.

---

## Known limitation: unbounded ACP session growth

A persistent ACP session accumulates every turn, and `session/load` replays the
whole history on each resume — O(history) work per mention, growing without
bound. This is acceptable for now (the reply fix hides the symptom; adapters cap
context internally) but is the main cost of the stateful default. Options if it
becomes a problem: periodic compaction, a max-age reset, or capping resumes and
falling back to a fresh+chat-recovery session.

---

## Future feature: chat inbox / proactive catch-up

Today an agent only acts when **mentioned** (push policy) — recovery is
mention-driven. A **chat inbox** would let an agent (especially a **pull-policy**
one, which the daemon never auto-launches) proactively answer "what mentions
have I not handled yet?" and catch up on its own cadence.

Sketch (not implemented):

- A per-agent "last handled" cursor over the chat (compare with the durable
  dedup in `src/agent/handled.rs`, which already records handled mentions).
- An API/CLI surface: e.g. `GET /circles/<id>/api/inbox?agent=claude` returning
  mentions of `@claude` newer than its cursor, or `enox agent inbox claude`.
- The agent (or a wrapper) polls the inbox, handles unaddressed mentions, and
  advances the cursor — turning "recover when mentioned" into "recover
  proactively."

This depends on chat persistence (the inbox reads the durable record) and dovetails
with the read-cursor idea deferred in
[control-persistence.md](control-persistence.md) §6 — they are the same cursor
concept from two angles (unread indicators for humans; unhandled mentions for
agents), and should be designed together.

---

## Open questions

- ACP session growth: leave unbounded, compact, or cap-and-recover?
- Should losing a session be surfaced to the user (e.g. "claude started fresh —
  its prior memory was unavailable"), or stay silent?
- Chat inbox: push (agent subscribes) vs. pull (agent polls) — and does the
  cursor live in the control doc (synced, visible) or per-device (private)?
