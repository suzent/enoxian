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

## `/enoxian/admin-handover/1.0.0`

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
header. See [the security model](../architecture/security.md) for the full boundary.

## Iroh Transport (in development)

Builds with `--features iroh-transport` can run a Circle on Iroh instead of
libp2p. Every device in the Circle must use the same transport. Set it per
Circle with `transport = "iroh"` in the Circle's `config.toml`, or for every
Circle of a daemon with `ENOXIAN_TRANSPORT=iroh`. The protocols above run
unchanged; only how their streams are carried differs.

- **Identity**: the endpoint key is the Circle's Ed25519 key, so a member's
  EndpointId and PeerId are the same public key and convert both ways.
- **Connections**: one QUIC connection per pair of peers, ALPN `enoxian/3`.
  The member with the lower PeerId dials; a joiner also dials the peers its
  invite names. If two connections appear, the one dialed by the lower PeerId
  stays.
- **Streams**: each bidirectional stream starts with a one-byte protocol tag
  (1 MLS bootstrap, 2 admin handover, 3 sync, 4 proposals, 5 events). Then
  both sides exchange a 32-byte proof that they hold the Circle secret:
  HKDF-SHA256 over the PSK, bound to the Circle id and both PeerIds. This
  replaces libp2p's pnet PSK, and unlike pnet it also covers relayed paths.
- **Admission**: a removed peer's connection is closed at once. Sync,
  proposals, events and admin handover also require a leaf in the local MLS
  group; MLS bootstrap needs only the proof, since that is how a joiner gets
  its Welcome. A refused stream is retried every 5 seconds while the
  connection lasts.
- **Relays**: `iroh_relays` in the Circle config lists relay URLs, as many as
  wanted. When it is empty, the Circle uses enoxian's relay
  (`DEFAULT_IROH_RELAYS`, `https://relay.enoxian.com`) together with Iroh's
  public relays in four regions (US east, US west, Europe, Asia-Pacific).
  Each device homes on the relay nearest it, and every dial lists all of
  them, since members of one Circle home on different relays. Nothing is
  published to Iroh's DNS address lookup. Iroh's public relays are rate
  limited (about 1 MiB/s relayed in our measurements) and can see which
  devices talk, though not what they say; list your own relays to avoid
  them.

An Iroh Circle mints v3 invites, which carry the transport and `iroh_relays`,
so a joining device sets itself up from the invite alone (see
[invites](invites.md#binary-format-v3)).

`ENOXIAN_TRANSPORT=iroh scripts/test-sync.sh` runs the two-daemon test on Iroh.
