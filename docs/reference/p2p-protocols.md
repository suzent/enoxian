# P2P Protocol Reference

Circle peers multiplex application streams over libp2p. Direct TCP circle
connections use the stable circle PSK, Noise identity authentication, and
Yamux. Relay circuits use Noise/Yamux through infrastructure that does not join
the circle. Content protocols add MLS-derived authenticated encryption.

## Content Frame

The v2 protocols wrap each logical payload as:

```text
magic[8] | version[1] | purpose[1] | epoch[8] | nonce[12] | ciphertext | tag[16]
```

- magic: `ENOXC17\0`
- version: `1`
- purpose: CRDT, proposal, or workspace event
- epoch: big-endian MLS epoch
- nonce: random 96-bit ChaCha20-Poly1305 nonce
- associated data: the complete header plus circle id

OpenMLS exports a 32-byte epoch secret with label `enoxian-content-v1`.
HKDF-SHA256 uses the circle id as salt and
`enoxian-content-frame-v1/<purpose>` as info. Keys therefore cannot be reused
across circles or protocol purposes.

Outer length/count fields remain visible for framing. Paths and semantic
payloads are encrypted together.

## `/enoxian/mls-bootstrap/1.0.0`

This persistent stream breaks the join/offline bootstrap cycle before a peer
has the current content key. It is protected by the circle PSK and Noise but is
not MLS-content-encrypted.

It carries only KeyPackages, signed owner/pending/member records, a Welcome
targeted at the receiver, removal tombstones, and the append-only MLS commit
sequence. It never carries files, chat, tasks, proposals, events, or blobs.

Peers periodically exchange changed membership snapshots. Offline retained
members replay missed commits and derive the current exporter secret. Daemons
retain eight recent exporter secrets in memory for in-flight old-epoch frames;
they are not persisted. A removed member can process its Remove commit but
cannot export the following epoch secret.

## `/enoxian/sync/2.0.0`

This persistent bidirectional stream carries Yjs file documents, the control
document, awareness, deletions, revocation/session messages, and live updates.

The initial exchange is deadlock-free:

```text
initiator: count + SyncStep1* -> SyncStep2* -> count + SyncStep1* -> SyncStep2*
responder: count + SyncStep1* -> SyncStep2* + count + SyncStep1* -> SyncStep2*
```

Each logical `(path, y-sync bytes)` pair is serialized inside one encrypted CRDT
frame. A dedicated reader prevents cancellation in the middle of a frame. If a
broadcast receiver lags, the sender transmits full idempotent CRDT state. P2P
updates use a `p2p` transaction origin to prevent echo.

## `/enoxian/proposals/2.0.0`

Once per connection, both peers reconcile their durable proposal stores:

```text
HAVE(id, fingerprint)* -> WANT(ids) -> BUNDLES -> WANT_BLOBS(hashes) -> BLOBS
```

The fingerprint covers mutable proposal status and result snapshot identity.
The deterministic status precedence resolves divergent decisions. Bundle and
blob messages are bounded, hash-verified, and encrypted as complete proposal
frames.

## `/enoxian/events/2.0.0`

Peers first exchange event ids, request the missing set, and send individually
bounded event envelopes followed by `EventsDone`. The stream remains open and
forwards newly appended events immediately.

Proposal-related events may carry a proposal bundle so metadata, manifests, and
ordinary blobs arrive with the decision. Event ids are immutable and merge by
set union; deterministic materialization resolves current state. The proposal
protocol remains useful for history reconciliation and missing large blobs.

## Authorization And Limits

All content protocols recheck removed-peer tombstones between phases and close
when either endpoint is removed. Frames are capped at 64 MiB. Content readers
wait briefly for bootstrap to install a requested MLS epoch, then fail closed.

Visible metadata includes peer routing identities, addresses, protocol choice,
connection timing, frame sizes/counts, traffic volume, and the MLS epoch/nonce
header. See [the security model](../concepts/security.md) for the full boundary.
