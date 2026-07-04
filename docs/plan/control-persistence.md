# Control-Doc Persistence — Design

**Status:** Design only. No implementation yet.

**Problem addressed:** the `__control__` CRDT (chat, tasks, member list,
presence, MLS state) is **in-memory only** — it is replicated to peers but never
written to disk on any device. If every member is offline and a daemon restarts,
that circle's chat/task/member history is **gone permanently**, because nothing
persisted it and no peer remains to re-sync from.

**Relationship to existing design:** this is the mirror image of
[proposal-pull-protocol.md](proposal-pull-protocol.md). That doc moved a
*durable, ever-growing* artifact (proposal history) *out* of the control doc
because the control doc is "designed for small, live coordination state
(chat/tasks/presence)." This doc asks the opposite question for the state that
remains: which of that "live coordination state" is actually durable, and how do
we persist it without recreating the unbounded-growth problem.

---

## 1. Current behavior (verified)

| Artifact | Key | Replicated? | Persisted to disk? | Preloaded on start? |
|----------|-----|-------------|--------------------|---------------------|
| Workspace files | (per-path docs) | yes | **yes** (`.enox_crdt/<path>`) | yes |
| Chat | `chat` | yes | **no** | no |
| Tasks | `tasks` | yes | **no** | no |
| Member list | `member_list` | yes | **no** | no |
| Presence | `presence` | yes | **no** | no |
| MLS state | `mls_*` | yes | partial (group state saved separately) | via sync |

The generic `crate::store::crdt::{save,restore}` exists and is used for file
docs; it is simply never called for `__control__`. So the mechanism is present;
only the policy of *what* to persist is missing.

**Consequence:** an all-offline restart loses chat, tasks, and any member-list
contributions that lived only in memory. Files survive; coordination state does
not.

---

## 2. The key finding: no delivery or read signal

None of the artifacts carry any per-member delivery or read state:

```text
ChatMessage { id, agent_id, text, mentions, ts }        // no read_by / delivered
Task        { task_id, title, status, claimed_by, ... }  // claimed_by is a weak
                                                          // "acted on", not "read"
MemberEntry { peer_id, owner, agents, role, ... }        // no per-member cursor
```

"Syncedness" is **transport-level and ephemeral**: CRDT state vectors decide
"does peer X's doc lack this update?" *at sync time*, and that fact is never
recorded on the artifact. The system can answer "is this replicated right now?"
but not "has member X received/read this message?".

**Why this matters for durability:** without a delivery/read signal you cannot
*safely prune* chat. Time-boxed retention ("keep last N days") is possible, but
its cost is explicit and permanent: **a member offline longer than the window
misses those messages with no way to recover them** — there is no per-member
"unread since" cursor to catch them up. Any pruning policy must own this
trade-off consciously.

This forks the design into two tiers, addressed separately below:

- **Tier A — persist what is safe today** (no read-signal needed).
- **Tier B — add a delivery/read signal** (a larger, optional follow-up).

---

## 3. Selective durability (Tier A)

Persist by artifact, not the whole doc — because the artifacts differ sharply in
value and in how they behave on restore.

### 3.1 Presence — **never persist**

Presence is inherently live state. Restoring "suzy was online 3 days ago" on
startup is actively wrong (ghost presence). Presence must always start empty and
be rebuilt by live heartbeats. This is non-negotiable and is the clearest reason
*not* to persist `__control__` wholesale.

### 3.2 Tasks and member list — **persist fully**

Both are bounded and genuinely valuable long-term:

- Tasks: the work queue is a system-of-record concern; losing it on a cold start
  is a real data loss. Bounded by the number of tasks, which is small.
- Member list: losing it means a rejoined circle forgets who its members are.
  Bounded by member count. (Note: entries are admin-signed, so persistence does
  not weaken trust — signatures are re-verified.)

### 3.3 Chat — **persist, time-boxed**

Persist chat so a cold-started circle is not empty, but bound growth by
retention window (e.g. last 30 days, or last N messages) rather than keeping it
forever. Accept the documented trade-off from §2: a member offline past the
window misses messages. This is the pragmatic middle between "ephemeral" (lose
everything on cold start) and "unbounded on-disk forever."

### 3.4 MLS state — **leave as-is**

MLS group state already persists via its own path (`group.save`); the `mls_*`
control-doc keys are delivery-service scratch (key packages, welcomes, commits)
and should follow the same lifecycle as the membership they serve. Out of scope
here; do not casually persist plaintext key material.

---

## 4. Mechanism (Tier A)

The control doc is a single Yjs doc with multiple top-level keys, so
"persist tasks+members+chat but not presence" cannot be done by saving the whole
doc. Two options:

**Option 4a — filtered snapshot on save.** On a debounced timer (and on clean
shutdown), build a *derived* doc containing only the durable keys (`tasks`,
`member_list`, a trimmed `chat`), encode it, and write to
`.enox_crdt/__control__`. On startup, restore it into the live control doc
*before* the swarm connects. Presence and MLS scratch are simply excluded.
Chat is trimmed to the retention window at save time (natural pruning point).

- Pro: one file, simple restore, pruning falls out of the save step.
- Con: rebuilding a filtered doc loses CRDT merge identity for the persisted
  keys — on restore we re-insert values, which is fine for these
  last-writer-ish maps but is not a true CRDT-state merge. Acceptable for
  tasks/members (keyed maps) and chat (append array), but must be validated
  against concurrent edits during the restore window.

**Option 4b — split `__control__` into sub-docs.** Give chat / tasks / members
their own Yjs docs, persist each with the existing per-doc `crdt::save`
unchanged, keep presence and MLS in an unpersisted doc. Truest CRDT semantics.

- Pro: reuses the file-doc persistence path verbatim; clean separation; presence
  never touches disk by construction.
- Con: larger refactor — every `get_or_insert_map(TASKS_KEY)` etc. moves to a
  different doc; sync must replicate multiple control docs.

**Recommendation:** start with **4a** (filtered snapshot) as the lower-risk
first step — it delivers the durability with a contained change and an obvious
pruning point. Revisit 4b only if concurrent-restore correctness proves fragile.

---

## 5. Interaction with the mention-replay fix

Persistence re-introduces a replay vector: on startup we now load chat *from
disk* in addition to *from peers*. The mention-reaction guards already added
(`src/agent/handled.rs` durable dedup + the `ts` cutoff in the reaction loop)
cover this — a restored old message is skipped by the `ts` cutoff, and even a
borderline-fresh one triggers at most once via the durable handled-set. **These
two features are now coupled:** the persisted chat and the persisted
`handled_mentions.log` must be reasoned about together, and a change to chat
retention must not silently drop a message whose handled-record was already
pruned (or vice versa). Document this coupling wherever chat pruning is
implemented.

---

## 6. Delivery / read signal (Tier B — optional, larger)

If we want smart pruning ("drop once every member has it") or unread indicators,
we need application-level delivery state, which the model lacks today.

Sketch, with honest costs:

- **Per-member read cursor.** `Map[member_id → last_seen_message_ts]` in the
  control doc. Enables "unread since" and safe pruning ("min cursor across
  members"). Cost: it is itself CRDT state that grows with membership and churns
  as members read — a smaller version of the same growth problem. Offline
  members never advance their cursor, so pruning stalls on the most-absent
  member (which is arguably *correct* — don't drop what someone hasn't seen —
  but means an abandoned device pins history forever; needs a staleness
  eviction rule).
- **Delivery vs. read distinction.** CRDT sync gives *delivery* implicitly
  (state-vector convergence) but recording it per-message is expensive. *Read*
  is a UI concern and only meaningful for human members, not agents.

**Recommendation:** defer Tier B. Ship Tier A with time-boxed chat first; only
add read cursors if unread indicators or delivery-based pruning become real
requirements. Note that read receipts interact with the M17 content-encryption
plan (who can see whose cursor) and should be designed alongside it, not before.

---

## 7. Proposed sequencing

1. **Tier A, Option 4a**, presence excluded, chat time-boxed. Persist on a
   debounced save + clean shutdown; restore before swarm connect.
2. Wire retention config (window length) into daemon config; default
   conservative (e.g. 30 days of chat).
3. Document the all-offline-recovery behavior and the no-read-signal trade-off
   in `docs/concepts/architecture.md` / `docs/concepts/security.md` (plaintext-at-rest note for
   the pre-M17 window).
4. Revisit 4b (sub-docs) only if 4a's restore semantics prove insufficient.
5. Tier B (read cursors) only on demand, designed with M17.

---

## 8. Open questions

- Retention window for chat: time-based, count-based, or both? Default?
- Is plaintext chat-at-rest acceptable before M17 content encryption, or should
  persistence wait for / integrate with it?
- Should tasks persist unconditionally, or also respect a retention/eviction
  rule for long-`Done` tasks?
- Option 4a's re-insert-on-restore vs. 4b's true CRDT merge — is the simpler
  path correct under a member editing concurrently during the restore window?
- Does an abandoned member's stale read cursor (Tier B) pin history forever, and
  what is the eviction rule?
