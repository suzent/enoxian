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

### 3.2 Framing (as implemented)

Each message is a single length-prefixed JSON value: `[u32 len][JSON]`, where the
JSON is a `Msg` enum — `Have(Vec<Have>)`, `Want(Vec<String>)`, or
`Bundles(Vec<ProposalBundle>)`. The exchange is three fixed round-trips
(HAVE → WANT → BUNDLES) rather than a streamed sequence with an END sentinel:
both sides know the next message kind, so no terminator is needed. `read_msg`
caps the frame at 64 MB to bound a malformed/hostile length prefix.

This is deliberately boring — JSON over a one-shot exchange, not a hot path. (It
diverges from the binary `(id)(fingerprint)` packing sketched earlier; JSON was
simpler and the volume is small. The §7 note on large histories still applies if
that ever changes.)

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

**Decision: full cutover in one change.** The control-doc map publish, observer,
and `PROPOSALS_KEY` are all removed in the same PR that adds the pull protocol —
no dual-path, no phased rollout.

Accepted consequence: a peer still on the **old** build exchanges proposals only
via the control-doc map, which new peers no longer write or read. So an
old↔new pair will not sync proposals until the old peer updates. This is
acceptable here because the circle is small and self-updated; it is the
trade-off for the simplest code and an immediate end to map growth. (File
content sync is unaffected — it runs on the separate `/enoxian/sync/1.0.0`
protocol.)

The disk store, `ProposalBundle`, `from_store`, and `apply_to_store` are reused
(with the §4 conflict-rule change to `apply_to_store`).

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

## 8. Implementation (single PR)

Shipped as one change:

1. **Model + conflict rule.** Add `updated_at` to `Proposal` (backfill
   `= created_at` on load via serde default), set it on every status change.
   Update `apply_to_store` to apply the §4 `(status_rank, updated_at)` rule
   instead of the current "changed if status differs" overwrite.
2. **Protocol.** New `src/network/proposal_sync.rs` implementing the
   HAVE/REQUEST/BUNDLE exchange (§3) with the delta computation as a pure,
   tested function. Wire `accept` + `open_stream` for `/enoxian/proposals/1.0.0`
   in `lifecycle.rs`, mirroring the `/enoxian/sync/1.0.0` setup, behind the same
   `mls_removed` tombstone gate.
3. **Remove the map path.** `publish_proposal` deleted; the `PROPOSALS_KEY`
   observer in `AppState::new` deleted; `PROPOSALS_KEY` removed from
   `control/mod.rs`. The engine and review API call sites that published to the
   map are dropped (the disk write remains; the new protocol replicates it).

---

## 9. Resolved decisions

- **Fingerprint** = `hash(status || result_snapshot_id)` (status-only). Enough:
  the only post-creation mutation is status, and the snapshot id changes if the
  content does.
- **Cadence** = once per connection, mirroring the CRDT catch-up. No timer / no
  local-change push.
- **Interim pruning** = not implemented. The pull protocol removes the growth
  problem directly, so the stopgap bounded-prune is skipped entirely.
- **Migration** = full cutover, one PR (see §6).
