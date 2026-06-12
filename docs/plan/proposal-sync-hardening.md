# Proposal Sync Hardening — Design Note

**Status:** §2 and §3 implemented; §1 still design-only. Companion to the
proposal layer in `agent-workspaces.md`. Covers the three risks left open after
proposal replication landed (`src/proposal/sync.rs`, control-doc `proposals`
map).

- **§2 (large-blob exclusion) — done.** `MAX_EMBEDDED_BLOB_BYTES` cap in
  `ProposalBundle::from_store`; reject/revert aborts cleanly on missing content
  (`src/api/proposals.rs`); CLI/diff show a "not synced" placeholder.
- **§3 (add/revert fold) — done.** Within-window fold pre-existed
  (`interactive_baseline`); the cross-window case is now closed by deferring
  emission of paths still being written and holding the burst baseline across
  windows (`classify_window` in `src/proposal/engine.rs`, with unit tests).
- **§1 (unbounded map growth) — open.** Still design-only; see below.

---

## 0. Background: where proposal state actually lives

This is the question that frames everything else, so it goes first.

There are **two stores**, and they are not the same thing:

| Store | What | Persistence | Role |
|-------|------|-------------|------|
| `<workspace>/.enox_proposals/` | proposal records, snapshot manifests, content blobs | **on disk** (`ProposalStore`, `src/proposal/store.rs`) | source of truth, survives restart |
| `__control__` Yjs map, key `proposals` | one `ProposalBundle` JSON per proposal | **in memory only** | replication channel between devices |

The review API and CLI read from the **disk** store. The control-doc map exists
only to carry bundles between peers: the publisher writes a bundle into it
(`publish_proposal`), and each peer's observer in `AppState::new` reads bundles
back out and writes them into its own **disk** store (`apply_to_store`).

### Why the control doc is in-memory and not saved

The `__control__` Yjs doc (chat, tasks, presence, members, mls_*, and now
`proposals`) is never passed to `crate::store::crdt::save` — that function is
called only for **file docs**, keyed by a relative path, from `flush_to_disk`
and the watcher. The control doc has no such call, by original design:

1. **It was conceived as live coordination state, not a database.** Presence is
   ephemeral by definition. Chat and tasks were treated as "rehydrate from peers
   on reconnect" rather than "persist locally" — a peer that restarts pulls
   current state from whoever it syncs with (the post-handshake catch-up sends
   full `__control__` state).
2. **CRDT merge makes rehydration safe.** Because the doc is a CRDT, pulling
   state from any peer and merging is idempotent and order-independent, so a
   restarted node converges without a local copy.
3. **It avoids a second persistence path.** File content already has a durable
   store (the file itself + `.enox_crdt/`); adding control-doc persistence was
   never on the critical path for the file-sync product.

### Why that is a latent problem now that proposals ride on it

The assumption in (1) breaks for proposals specifically:

- **Proposals are durable by intent.** A review history that vanishes when the
  last peer in a circle restarts is a bug, not a feature. Disk persistence
  (`.enox_proposals/`) already gives us this for the *local* store — good.
- **But replication depends on the in-memory map.** If every device restarts
  while a proposal's authoring device is offline, the bundle is gone from all
  live control docs. The authoring device still has it on disk, but it only
  re-publishes proposals as the engine *creates* them — it does **not**
  re-publish its existing disk store into a fresh control doc on startup. So a
  proposal can exist on disk on device A yet never reach device B if the timing
  is unlucky.
- **The map only grows.** Nothing prunes it (risk #1 below), and because it is
  in memory, every byte is paid again on every full resync.

The net: proposal *records* are durable locally, but proposal *replication* is
as ephemeral as chat — and proposals, unlike chat, are expected to be a
permanent, consistent-across-devices artifact. The hardening below addresses the
growth and size consequences; a separate open question (§4) is whether the
authoring device should re-publish its disk store on startup to close the
"everyone restarted" replication gap.

---

## 1. Risk: unbounded growth of the `proposals` map

**Current behavior.** `publish_proposal` inserts a bundle on every create and on
every status change; nothing is ever removed. A busy workspace accumulates
bundles without bound. Because the control doc is in memory and fully replicated
on connect, the cost is paid three times: resident memory on every device, the
full-state catch-up payload on every new connection, and per-bundle observer
work on apply.

**Recommendation.** Bound the map to the most recent N proposals (suggest
**N = 200**), pruning *terminal* proposals first (accepted / rejected / reverted)
and never pruning `pending` ones (those are the actionable review queue). Prune
in `publish_proposal` after insert.

**The subtlety.** Pruning a *replicated* CRDT map is not a local operation — a
`map.remove` propagates to every peer. Two concerns:

- **Resurrection.** If device A prunes bundle X while device B (offline) still
  has X and later reconnects, B's copy can re-insert X. CRDT maps have
  last-write-wins per key; a delete is itself an update, so whether X comes back
  depends on clock ordering. Pruning needs a tombstone or a "pruned below
  watermark" marker to be stable — otherwise pruning fights resurrection
  forever.
- **Disk vs. map divergence.** Pruning the map must **not** prune the disk store.
  The disk store is the durable record; the map is just transport. After
  pruning, the proposal still lists locally (CLI/API read disk) — it just stops
  being re-replicated. That is the correct semantics and should be explicit in
  the code + comment.

**Cheaper alternative.** Skip in-map pruning entirely; instead move proposal
replication off the control doc onto a dedicated request/response protocol
(M15's "content blob request/response over libp2p"), where a peer fetches only
the proposals it lacks rather than receiving the entire history on every
connect. Larger change, but removes the growth problem at the root instead of
managing it.

---

## 2. Risk: large-binary bloat

**Current behavior.** `ProposalBundle::from_store` base64-encodes every
referenced blob inline, with no size check. Base64 adds ~33%. A proposal
touching a 10 MB binary puts ~13 MB of string data into the control doc,
replicated to every peer and held in memory.

**Recommendation.** Add a per-blob size cap in `from_store` (suggest
**256 KB**). Blobs over the cap are **omitted** from `bundle.blobs`; the snapshot
manifest still records their hash + size, so the receiving device knows the file
changed and how big it is, but cannot render content. The diff view shows a
placeholder: `(large file — N bytes, content not synced)`.

**Consequences to handle:**

- `apply_to_store` already verifies each blob hashes to its key; an omitted blob
  simply isn't stored — the snapshot entry dangles. The review API's
  `store.blobs.get(...)` already returns `None` gracefully (it maps to
  `before: None` / `binary` today), so the render path needs only a clearer
  "not synced" vs. "binary" distinction.
- Reject/revert reverse-apply needs the base blob to restore content. If it was
  never synced, reject must fail cleanly on that device with "content not
  available on this device — review where the change originated," not silently
  corrupt the file. This is the most important correctness point in the whole
  note.
- This makes the M15 "missing-blob fetch on proposal receipt" item real: the
  proper fix is a pull protocol to fetch the large blob on demand when a device
  actually needs it (to view or revert), rather than pushing it to everyone
  eagerly.

---

## 3. Risk: add/revert produces two proposals across idle windows

**Current behavior.** The engine debounces edits with a 3 s `IDLE_WINDOW` and
advances its baseline each time a window closes with a non-empty diff. On the
device that *makes* a quick add-then-revert, both land in one window → net-zero
diff → no proposal. On a device that *receives* the two CRDT updates more than
3 s apart, window 1 sees the add (baseline advances), window 2 sees the revert
(now a non-empty diff against the advanced baseline) → two proposals. This is
the original symptom that began this work; attribution was fixed, this was not.

**Recommendation.** In the engine's idle flush, for interactive paths, compare
the resulting content against the content **at the proposal session's start
baseline**, not only the current (possibly already-advanced) baseline. If a path
has returned to its pre-session content, **fold** it (treat like a review
restoration) instead of emitting a proposal — even when an intervening window
advanced the baseline.

**The subtlety.** "Session start baseline" must be tracked separately from the
rolling baseline, or the fold check must reach back to the content the path had
before the *first* interactive touch in the current burst. The risk is
over-folding: a genuine A→B→A by a user who wanted both edits recorded would
collapse. Mitigate by scoping the fold to a short coalescing window (e.g. only
fold if both edits fall within one extended burst, not across minutes).

**Honest caveat.** This cannot be made perfect at the proposal layer — the true
fix is upstream batching of CRDT updates on the sending side so a single logical
edit doesn't arrive as two widely-spaced frames. The engine-side fold is a
mitigation, not a guarantee.

---

## 4. Open question: startup re-publication

Independent of the three risks: should the authoring device re-publish its
on-disk proposals into the control doc on daemon startup, so a circle that fully
restarted (all peers offline at once) still converges on the complete history?

- **For:** closes the replication gap described in §0; makes proposals as
  durable cross-device as they are locally.
- **Against:** re-publishing the whole disk store into the in-memory map on every
  start re-inflates exactly the growth problem from §1, and risks resurrecting
  proposals other devices intentionally pruned.

This interacts directly with §1's choice: if replication moves to a pull
protocol (the §1 alternative), startup re-publication becomes unnecessary —
peers fetch what they lack on demand. **Recommendation: decide §1 first.** If we
keep the control-doc map, add bounded re-publication (only `pending` +
recent terminal proposals). If we move to pull, drop this question.

---

## Suggested sequence

1. **§2 size cap** — smallest change, biggest safety win, no CRDT-ordering
   hazards. Includes the reject-without-blob correctness fix.
2. **§3 fold** — fixes the original user-visible bug; self-contained to the
   engine.
3. **§1 growth** — decide map-pruning vs. pull-protocol; this also resolves §4.
   The largest design commitment; worth its own note before coding.
