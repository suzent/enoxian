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

The agent's ACP session is the **authoritative** memory — it is not a duplicate
of the chat, and enoxian does not keep a parallel transcript. The circle chat is
only the **cold-start seed**: what a *fresh* session is given so it can catch up
when it has no memory to load.

| Surface | Role | Durability |
|---------|------|-----------|
| ACP session (agent-owned) | the memory; resumed by id | can be lost |
| Circle chat | cold-start seed for a fresh/recovered session | must persist |

This is why chat persistence matters here, not just for humans: the chat is what
a memory-less agent catches up from. Tracked as
[control-persistence.md](control-persistence.md) (M14.5) — without it, an agent
that loses its session on an all-offline restart has nothing to seed from.

---

## What this means for context injection (implemented)

`src/agent/context.rs::build_prompt` already follows the model:

- **Resumed session** — the agent holds the brief and prior turns in its own
  memory. `session/load` restores that memory **silently** — verified: the
  adapter emits no `session/update` replay between load and the prompt turn. So
  we send only a lean per-turn cue and **do not** re-inject the chat transcript;
  the agent already has the context, and re-feeding it wastes tokens.
- **Fresh session (also the recovery path)** — inject the full standing brief +
  recent chat so the agent catches up from the durable record, then the task.

**On the "greeting soup" bug (root-caused and fixed).** Earlier this was
misattributed to `session/load` replaying history. It was not — load is silent
(see above). The run-together greetings were the agent *genuinely* answering each
piece of injected context conversationally on a **fresh** session ("Hello! …" to
the brief, "Hi!" to a chat line) before doing the task. Two fixes:

1. **Prompt construction (root fix).** The cold-start context is now fenced in a
   `<context>` block prefaced by "Do NOT reply to it," and the prompt ends with a
   single labelled REQUEST. The agent treats the background as background and
   answers only the request. See *Prompt structure* in
   `src/agent/context.rs` (module docs) — verified: a fresh session now replies
   with just the answer, no greetings.
2. **Reply segmentation (belt-and-braces).** The capture still posts the agent's
   *final* message rather than a concatenation, so any stray preamble is dropped
   regardless.

---

## Session memory is agent-owned

The ACP agent (e.g. the managed `claude-agent-acp` bridge) owns the conversation state; enoxian only
persists the **session id** and hands it back via `session/load`. Same id →
same memory, restored on the agent's side. enoxian does not accumulate or replay
history itself — there is no enoxian-side transcript store for the agent beyond
the (separate) circle chat. So the earlier worry about "unbounded ACP history
replayed to us per mention" does not apply: growth and compaction are the
adapter's concern, and load is O(1) from enoxian's side.

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

- Should losing a session be surfaced to the user (e.g. "claude started fresh —
  its prior memory was unavailable"), or stay silent?
- Chat inbox: push (agent subscribes) vs. pull (agent polls) — and does the
  cursor live in the control doc (synced, visible) or per-device (private)?
