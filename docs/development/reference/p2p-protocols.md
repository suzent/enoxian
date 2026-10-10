# P2P Protocol Reference

Circle peers talk over Iroh: one QUIC connection per pair of peers, carrying
one bidirectional stream per protocol. Iroh's TLS handshake authenticates both
endpoint ids, every stream proves the Circle secret, and content protocols add
MLS-derived authenticated encryption on top.

0.11 and earlier used libp2p (PSK-gated TCP, Noise, Yamux, and
`/enoxian/...` protocol ids). 0.12 devices cannot talk to them.

## Transport

### Endpoint and identity

Each device runs one Iroh endpoint per Circle. Its key is the device's
per-circle Ed25519 key, so a member's EndpointId and PeerId are the same public
key and convert both ways. The endpoint publishes nothing to Iroh's DNS address
lookup; peers are dialed by id through the Circle's relays, plus any
direct-address hint from an invite.

### Connections

- ALPN: `enoxian/3`, the only one. It is visible in the QUIC handshake, so it
  is not a secret.
- One connection per pair of peers. The member with the lower PeerId dials; a
  joiner also dials the peers its invite named, since they do not know it yet.
  If two connections to one peer appear, the one dialed by the lower PeerId
  stays.
- A dial lists the peer's id, all of the Circle's relay URLs, and any direct
  addresses known for it. Iroh holepunches and migrates paths inside the one
  connection.
- An incoming connection from a removed peer is closed at once with reason
  `removed`. Removed peers are never dialed.
- Idle timeout 15 seconds, with keep-alives every 5. The dialer redials 2
  seconds after losing a peer, and sweeps all members every 30 seconds with
  per-peer backoff.
- When a peer's sync session ends, the connection is closed so the dialer
  reconnects and every stream starts over. The close waits a second first:
  closing a QUIC connection discards data the peer has not acknowledged, and
  a session that ends on purpose ends with a last frame (`REVOKED`).
- Either side may open a stream on a connection, whoever dialed it: admin
  handover opens from the leaving admin.

The dialer opens MLS bootstrap, sync, proposals and events, in that order, as
soon as a connection is up. Admin handover is opened on demand.

### Stream header

Every bidirectional stream starts with a protocol tag and a Circle proof:

```text
opener   → acceptor   tag[1] | proof[32]
acceptor → opener     proof[32]
```

| Tag | Protocol |
|-----|----------|
| 1 | MLS bootstrap |
| 2 | Admin handover |
| 3 | Sync |
| 4 | Proposals |
| 5 | Events |

Any other tag is refused. The proof is:

```text
HKDF-SHA256(salt = "enoxian-circle-proof-v1", ikm = Circle PSK,
            info = len32 ‖ circle_id ‖ len32 ‖ sender PeerId ‖ len32 ‖ receiver PeerId)
```

`len32` is a 4-byte big-endian length; PeerIds are in their binary form. Both
PeerIds come from the authenticated connection, so a proof cannot be replayed
to another peer or reflected back. Proofs are compared in constant time.

The acceptor checks the proof, and for every protocol except MLS bootstrap
also checks that the opener has a leaf in its MLS group. Only then does it
send its own proof. On failure it resets the stream. The opener checks the
returned proof the same way. The whole header must complete within 10
seconds. An opener whose stream was refused retries every 5 seconds while the
connection lasts, which covers a joiner that has not been admitted yet or a
peer that is behind on MLS.

After the header, the stream carries the protocol's own framing, below.

## Content Frame

The v2 protocols wrap each logical payload as:

```text
magic[8] | version[1] | purpose[1] | epoch[8] | nonce[12] | ciphertext | tag[16]
```

- magic: `ENOXC17\0`
- version: `1`
- purpose: CRDT, proposal, workspace event, or admin handover
- epoch: big-endian MLS epoch
- nonce: random 96-bit ChaCha20-Poly1305 nonce
- associated data: the complete header plus circle id

OpenMLS exports a 32-byte epoch secret with label `enoxian-content-v1`.
HKDF-SHA256 uses the circle id as salt and
`enoxian-content-frame-v1/<purpose>` as info. Keys therefore cannot be reused
across circles or protocol purposes.

Outer length/count fields remain visible for framing. Paths and semantic
payloads are encrypted together.

## MLS bootstrap (tag 1)

This persistent stream breaks the join/offline bootstrap cycle before a peer
has the current content key. It requires the Circle proof but no MLS leaf, and
is not MLS-content-encrypted.

It carries only KeyPackages, signed owner/pending/member records, a Welcome
targeted at the receiver, removal tombstones, and the append-only MLS commit
sequence. It never carries files, chat, tasks, proposals, events, or blobs.

Peers periodically exchange changed membership snapshots. Offline retained
members replay missed commits and derive the current exporter secret. Daemons
retain eight recent exporter secrets in memory for in-flight old-epoch frames;
they are not persisted. A removed member can process its Remove commit but
cannot export the following epoch secret.

## Admin handover (tag 2)

A one-shot stream a leaving admin opens to its chosen successor. Both frames are
content frames with purpose `admin handover`, each behind a 4-byte big-endian
length prefix and capped at 64 KiB:

```text
leaver → successor   { circle_id, admin_key_hex }
successor → leaver   { ok, error }
```

The successor accepts only from a current member, only for its own circle, and
only a key whose public half matches its pinned `admin_pubkey_hex`. It writes
`admin.key` with owner-only permissions before it replies `ok`. The leaver deletes
nothing until that reply arrives, and gives up after 30 seconds.

## Sync (tag 3)

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

Reserved paths (prefixed with a NUL byte, so they cannot collide with a real
file) carry control messages on the same stream: awareness, deletions,
revocation, session hello, and chat attachment transfer.

### Chat attachment transfer

Chat attachments ride this stream rather than the proposal protocol, because
proposal reconciliation runs only once per connection — an image posted
mid-session would otherwise not reach peers until the next reconnect.

```text
\0blob-want/<sha256>   ->   \0blob-data/<sha256> + raw bytes
```

A want is broadcast to every connected peer; any of them may answer, and a
duplicate answer is discarded. A peer serves only blobs it actually holds, and
silence is a valid response. Received content is rejected unless it hashes to
the name it arrived under, so a peer cannot substitute different bytes for an
attachment another member posted. Frames are binary-safe, so unlike proposal
bundles the payload needs no base64 expansion.

On stream setup each side re-requests any attachment still missing from its
transcript, which backfills a device that was offline or joined late.

## Proposals (tag 4)

Once per connection, both peers reconcile their durable proposal stores:

```text
HAVE(id, fingerprint)* -> WANT(ids) -> BUNDLES -> WANT_BLOBS(hashes) -> BLOBS
```

The fingerprint covers mutable proposal status and result snapshot identity.
The deterministic status precedence resolves divergent decisions. Bundle and
blob messages are bounded, hash-verified, and encrypted as complete proposal
frames.

## Events (tag 5)

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

A relay or network observer sees endpoint ids, addresses, the ALPN,
connection timing, packet sizes and counts, and traffic volume. Tags, proofs
and frames travel inside the encrypted QUIC connection; the receiving peer
also sees the MLS epoch/nonce header. See [the security model](../architecture/security.md) for the full boundary.

## Relays

A Circle's relays are its `iroh_relays` config when that lists any. When it is
empty, the Circle uses enoxian's relay (`DEFAULT_IROH_RELAYS`,
`https://relay.enoxian.com`) together with Iroh's public relays in four
regions. Each device homes on the relay nearest it, and every dial lists all of
them, since members of one Circle home on different relays. Iroh's public
relays are rate limited (about 1 MiB/s relayed in our measurements) and can see
which devices talk, though not what they say; list your own relays to avoid
them. Invites carry the inviter's `iroh_relays` (see
[invites](invites.md#binary-format-v3)).

`scripts/test-sync.sh` runs two daemons against each other over Iroh.
