# Proposal Pull Protocol — Design (§1 root fix)

**Status:** Design only. Addresses §1 of `proposal-sync-hardening.md` (unbounded
growth of the control-doc `proposals` map). This is the "cheaper root-fix: move
to a pull protocol" route — it removes the growth problem instead of managing it
with in-map pruning.

**Relationship to the roadmap:** this is the concrete realization of M15's
"Content blob request/response protocol over libp2p" and "Missing-blob fetch on
proposal receipt," scoped to proposals.

---

## 1. Problem recap

Today every proposal is published as a `ProposalBundle` JSON into the
`__control__` Yjs map under `PROPOSALS_KEY` (`src/proposal/sync.rs`). That map:

- is **in memory** and never pruned, so it grows without bound;
- is **fully replicated on every connect** (the post-handshake catch-up in
  `sync_inner` sends complete `__control__` state), so the entire proposal
  history — bundles, snapshots, base64 blobs — is re-sent on every reconnect;
- couples a durable, ever-growing artifact (review history) to a transport that
  was designed for small, live coordination state (chat/tasks/presence).

The disk store (`.enox_proposals/`) is already the durable source of truth and
is read by the CLI/API. The map is *only* a replication channel. So the fix is
to replace that channel with one that transfers **only what a peer is missing**,
on demand, rather than pushing the whole history to everyone eagerly.

---

## 2. Goal and non-goals

**Goal.** A peer obtains every proposal that exists in the circle without the
control doc carrying proposal data. Convergence must be eventual and not depend
on any peer staying online.

**Non-goals.**
- Not changing the on-disk store format.
- Not changing the proposal/bundle data model (the `ProposalBundle` payload is
  reused verbatim as the transfer unit).
- Not solving large-binary transfer beyond what §2's size cap already does — a
  bundle still excludes oversized blobs; this protocol transfers bundles, so the
  same exclusion applies. (A future extension can add per-blob fetch.)

---

## 3. Mechanism: a second stream protocol

The daemon already multiplexes app protocols over `libp2p_stream`
(`stream::Behaviour`). The CRDT sync runs on `/enoxian/sync/1.0.0`
(`src/network/sync.rs`), accepted and opened in `src/lifecycle.rs` via
`stream_control.accept(PROTOCOL)` / `open_ctrl.open_stream(peer, PROTOCOL)`.

Add a sibling protocol:

```
/enoxian/proposals/1.0.0
```

wired the same way — one `accept` task and one `open_stream` call per peer
connection — so no new `NetworkBehaviour` is needed.

### 3.1 Exchange (anti-entropy, both directions)

On each connection both sides run a symmetric reconciliation. Using **proposal
ids** as the unit:

```text
1. Each side reads its local id set from the disk store
   (cheap: list_proposals() already exists; we need ids + a content fingerprint).
2. Initiator sends: HAVE { (id, fingerprint) for all local proposals }
3. Responder computes the delta:
     - ids the responder lacks         -> responder will REQUEST them
     - ids the responder has but with a
       different fingerprint (status
       changed)                        -> responder will REQUEST them
     - ids the responder has that the
       initiator lacks                 -> responder offers them in its own HAVE
4. Responder sends its HAVE; symmetric delta is computed on the initiator.
5. Each side REQUESTs the ids it needs; the other STREAMs those bundles.
6. Each received bundle is applied via ProposalBundle::apply_to_store (unchanged).
```

The **fingerprint** is what makes status changes (accept/reject/revert)
propagate without re-sending everything: define it as a hash of the
status-bearing fields, e.g. `hash(status || result_snapshot_id)`. Two peers
agree on a proposal iff ids and fingerprints match; otherwise the newer one wins
(see §4 conflict rule).

### 3.2 Framing

Reuse the existing length-prefixed frame helpers from `sync.rs`
(`write_frame`/`read_frame`) or a small local equivalent:

```text
HAVE      = [u32 count][ (36-byte id)(8-byte fingerprint) ]*
REQUEST   = [u32 count][ (36-byte id) ]*
BUNDLE    = [u32 len][ ProposalBundle JSON ]      (one per requested id)
END       = zero-length frame to close a direction
```

Keep it deliberately boring; this is not a hot path.

---

## 4. Conflict rule (status divergence)

Two devices can change the same proposal's status concurrently (e.g. one accepts
while another rejects, offline). The synced-map version inherited Yjs
last-write-wins; the pull protocol must define this explicitly.

**Rule.** Per proposal id, the winning record is the one with the greater
`(status_rank, updated_at)` where:

- proposals gain an `updated_at` timestamp (new field; defaults to `created_at`
  for legacy records), set whenever status changes;
- `status_rank` breaks ties deterministically so all peers converge to the same
  choice regardless of clock skew — suggested order: `reverted > rejected >
  accepted > pending` (a terminal decision beats a pending one; among terminals,
  a later explicit action wins by `updated_at`).

This is a behavioral change worth calling out for review: today divergent
decisions resolve by Yjs clock; here they resolve by an explicit, auditable
rule. `apply_to_store` must be updated to apply this rule instead of the current
"changed if status differs" overwrite — i.e. **do not** blindly overwrite a
local terminal status with an inbound one of lower rank.

---

## 5. What gets removed

- `publish_proposal` no longer writes to the control doc map.
- The `PROPOSALS_KEY` observer in `AppState::new` is removed (or repurposed only
  for migration — see §6).
- `PROPOSALS_KEY` is retired from the control doc once migration is complete.

The disk store, `ProposalBundle`, `from_store`, and `apply_to_store` are all
**reused unchanged** (modulo the §4 conflict-rule tweak to `apply_to_store`).

---

## 6. Migration / backward compatibility

Peers on the old build only replicate via the control-doc map; peers on the new
build only via the pull protocol. A mixed circle must still converge.

**Plan: ship the pull protocol additively first.**

1. **Phase 1 (this design's first PR).** Add the pull protocol and run it
   *alongside* the existing map publish/observe. New peers exchange via the
   protocol; old peers still get map updates. The map is still written, so
   nothing regresses — but new↔new pairs no longer *depend* on it. Growth is not
   yet fixed (map still written), but the mechanism is proven.
2. **Phase 2.** Once all peers are known to run Phase 1+, stop writing the map
   (`publish_proposal` becomes pull-only) and drop the observer. This is the
   change that actually fixes growth. Gate it behind a protocol-version check or
   a config flag so a straggler old peer degrades to "no proposal sync with new
   peers" rather than silent divergence.
3. **Phase 3 (optional).** Remove `PROPOSALS_KEY` entirely.

The two-phase split keeps each PR small and reversible, and means the risky part
(removing the eager map) ships only after the new path is validated in the wild.

---

## 7. Edge cases

- **Empty/None blobs.** Already handled by `apply_to_store` (verifies hash before
  storing); unchanged.
- **Oversized blobs.** Bundle already excludes them (§2). A pulled bundle for a
  large file lands as manifest-only; reject/revert on that device already aborts
  cleanly (§2 fix). No new handling needed here, but a *future* per-blob pull
  could fetch the content on demand — out of scope.
- **Evicted peers.** The pull protocol must sit behind the same `mls_removed`
  tombstone gate as `sync_inner` — an evicted peer must not be able to pull
  proposals. Reuse that check at the top of the accept handler.
- **Large histories.** The HAVE set is `(id, fingerprint)` pairs — 44 bytes
  each, so even 10k proposals is ~440 KB exchanged once per connect, vs. the
  current full-bundle resend. If that becomes a concern, HAVE can be chunked or
  summarized with a Merkle/rolling digest, but that is a later optimization.

---

## 8. Suggested PR breakdown

1. **Add `updated_at` + status_rank conflict rule** to the model and
   `apply_to_store`, with tests. Self-contained; improves correctness even
   before the protocol lands. Backfills `updated_at = created_at` on load.
2. **Add the `/enoxian/proposals/1.0.0` protocol** (accept + open wiring in
   `lifecycle.rs`, a new `src/network/proposal_sync.rs` with the HAVE/REQUEST/
   BUNDLE exchange), running alongside the map (Phase 1). Tests for the delta
   computation as a pure function.
3. **Flip to pull-only** (Phase 2): stop writing the map, drop the observer,
   behind a version/flag gate.
4. **Remove `PROPOSALS_KEY`** (Phase 3) after a release cycle.

PR 1 is independently useful and low-risk; it can land first regardless of the
rest. PR 2 is the bulk of the work. PR 3/4 are cleanups gated on rollout.

---

## 9. Open questions for review

- **Fingerprint definition.** Is `hash(status || result_snapshot_id)` enough, or
  do we want the full bundle hash so any field drift triggers a refetch? Full
  hash is simpler to reason about but refetches more often.
- **Trigger cadence.** Reconcile once per connection (like the CRDT catch-up),
  or also on a timer / on local change? Once-per-connect is simplest and matches
  current semantics; local-change push-notify is a latency optimization.
- **Do we keep the §1 interim pruning at all?** If the pull protocol lands soon,
  in-map pruning is wasted work. If the protocol slips, a small bounded prune
  (note §1 "Recommendation") is worth shipping as a stopgap. Recommendation:
  skip interim pruning unless PR 2 is deprioritized.
