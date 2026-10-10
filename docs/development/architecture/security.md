# Security Model

This document describes the current security model. The implementation combines
stable device identity, an authenticated Iroh transport, a per-stream Circle
proof, and MLS-derived content keys. The Circle PSK is stable; it is not derived
again for every MLS epoch.

## Trust Boundaries

enoxian separates transport, identity, membership, and content:

| Layer | Mechanism | What it proves | Status |
|-------|-----------|----------------|--------|
| Identity | Iroh QUIC (TLS 1.3) keyed by the per-circle Ed25519 key derived from the device key | The peer owns this endpoint id, which is its peer ID | Implemented |
| Circle | Circle proof on every stream, derived from the stable PSK | The peer holds the circle secret | Implemented |
| Membership | Removed peers refused at connection; MLS leaf required on content streams; signed member list + `mls_removed` tombstones | The peer is a current member and has not been evicted | Implemented |
| Content | MLS exporter + HKDF + ChaCha20-Poly1305 | The peer holds the active MLS epoch secret and the frame was not modified | Implemented |

Relays and the bootstrap server are outside the circle trust boundary. They
join no circle and hold no circle PSK.

0.11 and earlier used libp2p, with the PSK applied through `libp2p::pnet` on
direct TCP only and Noise for identity. 0.12 removes libp2p entirely; the two
cannot talk to each other.

## Transport

Every circle runs one Iroh endpoint per device. The endpoint's secret key is the
device's per-circle Ed25519 key (see *Peer Identity*), so a member's Iroh
endpoint id and its peer ID are the same public key. Iroh connections are QUIC
with TLS 1.3, and the TLS handshake authenticates both endpoint ids. After the
handshake each side knows, with cryptographic certainty, which peer ID is on the
other end.

One QUIC connection per pair of peers carries every protocol, under the single
ALPN `enoxian/3`. The connection may run over a direct UDP path or through a
relay; Iroh holepunches and moves between paths inside the same connection. The
security properties below are the same on every path.

The endpoint publishes nothing to Iroh's DNS address lookup, and port mapping
(UPnP/NAT-PMP) is disabled. Members find each other by peer ID through the
circle's relays, plus any direct-address hint an invite carried.

## Circle Proof

Every circle has a stable 256-bit pre-shared key. It is never sent on the wire.
Instead, every stream opens with a one-byte protocol tag and a 32-byte proof in
each direction:

```text
proof = HKDF-SHA256(salt = "enoxian-circle-proof-v1",
                    ikm  = circle PSK,
                    info = len ‖ circle_id ‖ len ‖ sender peer ID ‖ len ‖ receiver peer ID)
```

Both peer IDs come from the TLS-authenticated connection, so a proof made for
one peer cannot be replayed to another, or in the other direction. Proofs are
compared in constant time. A stream that does not show its tag and proof within
10 seconds is reset.

The opener sends its tag and proof first. The accepting side checks the proof,
applies the admission rules below, and only then sends its own proof back, so a
peer that fails learns nothing about the secret.

The proof replaces libp2p's pnet. Unlike pnet, which guarded direct TCP only, it
covers every path, including relayed ones.

The PSK is a coarse gate, not the revocation mechanism. It is distributed in
invite links and saved in `~/.enoxian/circles/<id>/config.toml`. It does not
expire after a peer joins, and it is not rotated on MLS epoch changes.

## Admission

Admission is decided per connection and per stream:

1. **Connection.** A device whose peer ID is tombstoned in `mls_removed` has its
   incoming connection closed at once (reason `removed`). Members never dial a
   removed peer.
2. **Every stream.** The Circle proof must verify.
3. **Content streams** (sync, proposals, events, admin handover). The peer must
   also have a leaf in our MLS group. A peer that is not in the group yet, or
   that we have not caught up on, is refused; the opener retries every 5
   seconds while the connection lasts.
4. **MLS bootstrap** needs only the proof, because it is how a joiner obtains
   its Welcome. What it may exchange there is limited (see *Content Frames*).

## Peer Identity

Each install has one stable device key in `~/.enoxian/identity.toml`.
Per-circle connection keypairs are derived deterministically from that device key
using HKDF-SHA256:

```text
HKDF(device_key, "enoxian-device-v1", "circle/<circle-id>")
```

The result is a stable peer ID for each `(device, circle)` pair. The same key is
the device's Iroh endpoint key, and the QUIC/TLS handshake proves ownership of
it during connection setup.

Implications:

- A peer cannot impersonate another peer ID without the corresponding key.
- Rejoining the same circle from the same device presents the same peer ID.
- A removed peer that reconnects with the same identity is refused at
  connection, by every member that has its tombstone.

## Membership And Eviction

The member list (`member_list` in the control CRDT doc) is the replicated
directory of peer IDs, roles, owners, and agent labels. Mutating member
operations require an admin signature.

Eviction is enforced by `mls_removed`:

1. The admin removes a member.
2. The member entry is removed and a tombstone is written to the `mls_removed`
   CRDT map.
3. The MLS Remove commit is broadcast through `mls_commits` so remaining members
   keep MLS membership state in sync.
4. Members close any connection from a tombstoned peer and stop dialing it.
   `src/network/sync.rs` also checks `mls_removed` before exchanging any CRDT
   data and rejects tombstoned peers.

The MLS Remove commit advances the content-encryption epoch. A removed member
can process the removal but cannot export the new epoch secret, so it cannot
decrypt subsequent content frames. Tombstones remain a useful early gate at
connection and sync; the MLS epoch is the cryptographic boundary.

## MLS

enoxian uses IETF MLS (RFC 9420), implemented with `openmls`, for group
membership cryptography and content-layer encryption.

MLS commits are replicated in the control doc:

- `epoch` — the post-commit epoch
- `data_hex` — TLS-serialized `MlsMessageOut`
- `sender_peer_id` — who issued the commit
- `ratchet_tree_hex` — ratchet tree extension data for joins

Each daemon applies incoming commits serially and retains a small in-memory
window of exporter secrets for frames already in flight. Offline members replay
the durable commit sequence before opening content from a newer epoch. The MLS
exporter secret is never used as the circle PSK.

## Content Frames

The v2 sync, proposal, and workspace-event protocols encrypt every logical
payload with ChaCha20-Poly1305. The frame header contains a fixed magic value,
format version, purpose, MLS epoch, and random nonce. The header and circle ID
are authenticated as associated data. HKDF-SHA256 domain-separates CRDT,
proposal, and event keys from the MLS exporter secret, preventing ciphertext
from being moved between protocol purposes or circles.

A separate MLS bootstrap stream solves the join/offline bootstrap cycle. It
carries only KeyPackages, signed owner/pending/member records, targeted
Welcomes, removal tombstones, and MLS commits. It requires the Circle proof but
not the content key, because a joiner does not have that key yet. Anyone who
holds the PSK can reach it, so a peer is treated by its MLS group membership:
one with a leaf in our group exchanges every record, and one without receives
only our own entries plus its own Welcome and tombstone, and may write only
entries about itself. MLS commits are applied from anyone, since MLS verifies
them. The stream never carries workspace files, chat, tasks, proposal content,
event-log entries, or blobs.

## Current Attacker Capabilities

### Current Member With The PSK

| Action | Possible? |
|--------|-----------|
| Connect to other members | Yes |
| Read synced workspace files and chat | Yes |
| Write CRDT updates | Yes |
| Impersonate another peer ID | No, the QUIC/TLS handshake proves key ownership |
| Perform admin member operations | No, requires `admin.key` |

### Removed Member Who Still Has The Stable PSK

| Action | Possible? |
|--------|-----------|
| Connect to members that have its tombstone | No, they close the connection and never dial it |
| Open content streams with members whose MLS group has removed it | No, it has no leaf |
| Read new CRDT/proposal/event content after the removal epoch | No |
| Read data already synced to local disk | Yes |
| Read bootstrap membership records and MLS commits | No, only the sender's own records and its own tombstone once out of the MLS group |

### Invite Holder Not Yet Admitted

| Action | Possible? |
|--------|-----------|
| Open the MLS bootstrap stream | Yes, it holds the PSK |
| Open content streams | No, it has no MLS leaf until an admin admits it |
| Read bootstrap membership records | No, only each member's own records |
| Write membership records about other peers | No, only about itself |
| Read circle content | No |

### Outside Peer Without The PSK

| Action | Possible? |
|--------|-----------|
| Complete a QUIC handshake with a member it can address | Yes, with its own key |
| Open any stream | No, every stream fails the Circle proof |
| Read bootstrap membership records | No |
| Read circle content | No |
| Discover members on the LAN | No, there is no LAN discovery and nothing is published to DNS |

## Invites

Invite links contain the circle ID, the stable PSK, an expiry timestamp, an
optional circle name, the inviter's peer ID with an optional direct address,
optional Iroh relay URLs, and an optional admin public key.

Expiry is enforced by `enox enter` before joining. It prevents accidental or
late use of old links, but it does not revoke a peer that already joined and
saved the PSK.

Practical guidance:

- Share invite links only over trusted channels.
- Use short TTLs for one-off onboarding.
- Remove unwanted members promptly so the tombstone propagates.
- Treat the PSK as a durable secret until explicit circle-key rotation is added.

## Relays And The Bootstrap Server

### Default Relays

A circle whose config lists no `iroh_relays` uses the defaults: enoxian's relay
(`https://relay.enoxian.com`, `DEFAULT_IROH_RELAYS` in `src/defaults.rs`) plus
Iroh's public relays, run by n0, in four regions. Each device homes on the relay
nearest it, and peers reach it there until a direct path opens. A device behind
symmetric NAT (a mobile carrier, for example) never gets a direct path, so its
traffic stays on a relay.

The practical consequence is that the default posture contacts third-party
infrastructure. To avoid it, run your own relay (`enox bootstrap serve
--iroh-relay`, see [rendezvous setup](../../guide/rendezvous-setup.md)) and list
it in the circle's `iroh_relays`. A configured list replaces the defaults
entirely.

### What A Relay Sees

A relay forwards QUIC packets between endpoints. It cannot read them: the QUIC
connection is end-to-end between the two members, and content frames are
additionally MLS-encrypted inside it. The relay does see:

- the endpoint ids of both sides, which are the members' peer IDs;
- their IP addresses, timing, packet sizes, and traffic volume;
- the ALPN `enoxian/3`, which travels in the clear in the QUIC handshake.

The ALPN is therefore not a secret. It identifies enoxian traffic, nothing more;
the Circle proof is what keeps outsiders out.

### The Bootstrap Server

`enox bootstrap serve` holds no PSK and joins no circle. It serves HTTP for
version and upgrade checks, the `enox link` pairing dead drop, and sealed
short-invite blobs, and with `--iroh-relay` it also runs an Iroh relay with the
properties above. It no longer does peer discovery or libp2p circuit relay. It
learns metadata such as which clients fetch from it and when, but it does not
see circle sync traffic except as a relay of encrypted QUIC.

## Residual Metadata Leakage

Content encryption protects payloads, not traffic shape. A relay or network
observer can still learn the peer IDs (endpoint ids) of both sides, IP/address
information, connection timing and duration, that the traffic is enoxian
(from the ALPN), packet sizes and counts, and traffic volume. Protocol tags and
proofs travel inside the encrypted QUIC connection. The bootstrap stream
additionally exposes each member's own membership records to any peer that
holds the PSK. MLS epochs and nonces are visible in encrypted frame headers to
the receiving peer. File paths are inside the encrypted CRDT frame and
proposal/event metadata and blobs are encrypted as one authenticated payload.

## What Agents Send Out

An agent is a model provider's CLI running on someone's machine, so anything it
reads leaves the Circle. Two settings change how much, and both are deliberately
per device, in the never-synced `agents.toml`: the device that pays for an
agent's tokens is the device that decides what they are spent on. A remote peer
cannot opt another device's agent into anything.

**Mentions (default).** Chat reaches a provider only when someone deliberately
summons an agent, plus the recent-context window that mention carries.

**Follow-up routing.** The same, for a few minutes after a reply, without the
mention. It changes when an agent is woken, not what it can see.

**Ambient (`engagement = "ambient"`, off by default).** Every human line in the
Circle is sent to that agent's provider — possibly several vendors at once, for
the same sentence. That is a defensible trade for a working Circle and an
unpleasant surprise for a social one, so the roster advertises
`ambient_agents`: every peer can see which agents on which devices are reading
the room, not just the device that configured one.

**Delegation (`accept_from = "agents"`, off by default).** Lets one agent's
reply wake another, which means a message can reach a provider nobody addressed
it to. Bounded by the relay budget, and the switch is on the receiving side.

A forged `relay` chain on the wire is not an escalation. Enforcement is local:
the receiving device clamps the budget to its own configuration and keeps its
own per-cascade count, so a hostile peer inflating `spent` buys nothing it could
not already get by posting a mention in a loop.

## Local Daemon API

The managed daemon also exposes a local HTTP/WebSocket API for the CLI and web UI. This API
is a control plane, not the WAN relay. It can read and mutate circle state, so it
must be treated as privileged local infrastructure.

The HTTP/WebSocket listener binds to loopback by default. Every API request
requires the bearer token stored at `~/.enoxian/api.token`, and CORS permits only
local origins. Explicit LAN binding widens the attack surface and should be used
only on a trusted network.

External agents can obtain a separate short-lived actor token with `enox
register`. Actor tokens are opaque random bearer values bound in daemon memory
to one Circle, one agent label, and the local cryptographic peer ID. They improve
attribution and prevent cross-device replay, but they do not create isolation
between processes on the same device: any local process able to read a token can
act under that label. Passing `--token` can also expose it through process lists
or tool logs. Actor tokens therefore expire after one hour, disappear on daemon
restart, and confer no membership or administrative authority.

Enoxian-managed agents use the same token validation for coordination commands,
but registration and transport are automatic: the token is inherited by the
managed process tree and read by the CLI. Native file tools cannot attach a
token to each write, so the proposal engine instead correlates watcher events
with the single persisted managed-process session. The session records actor,
Circle, start/end times, and verified-process confidence; it grants no extra
authority and does not place the bearer token in model context.

## Admin Key

The circle creator generates an Ed25519 admin keypair at `enox init`. The
private key (`admin.key`) remains on the creator's machine; the public key is
embedded in invites so peers can verify admin operations.

Only the holder of `admin.key` can add, remove, approve, reject, or promote
members. If the admin private key is compromised, an attacker can perform member
operations. Admin key rotation and multi-admin recovery are not yet implemented.

An admin that runs `enox leave` hands the key to a connected member first (see
[`/enoxian/admin-handover/1.0.0`](../reference/p2p-protocols.md)). The key
travels inside a content frame, so only a member holding the current MLS epoch
can read it. The recipient keeps it only if its public half is the admin key the
circle already pins, so no member can be made to trust a new key. The same key
stays in use, which means the leaving device could have kept a copy. A lost or
broken admin device cannot hand anything over, and its circle loses admin.

## LAN Exposure

enoxian does no LAN discovery; 0.11 and earlier announced peer IDs over mDNS,
and 0.12 does not. Iroh may still find and use a direct LAN path between two
members that are already connected. Observers on the LAN see the same metadata
as any network observer (see *Residual Metadata Leakage*), not content or the
PSK.

## Data At Rest

Circle content is stored **unencrypted** on each device:

- Workspace files live in the workspace directory as plain files.
- CRDT state (per-file docs) is persisted under `.enox_crdt/`.
- Coordination state — chat (last 30 days), tasks, and the member list — is
  persisted to `<circle_dir>/control.json` so it survives an all-offline restart.
  Chat is written **plaintext**.

Message-layer encryption deliberately does not alter native file IO
or local persistence. Anyone with filesystem access to a member's device can
still read that circle's content. Treat local disk as trusted and use host
full-disk encryption where this is a concern.

## Secrets At Rest

Several files under `~/.enoxian` are secrets in the plain sense that anyone who
reads them can be you:

| File | Holds |
|------|-------|
| `identity.toml` | The device seed, and the recovery phrase on the device that created the user identity |
| `circles/<id>/config.toml` | That circle's PSK and this device's per-circle private key |
| `circles/<id>/admin.key` | The admin signing key, on an admin machine |

All three are written `0600` through a temporary file that is renamed into place — so the mode comes from the new file rather than from a `chmod` on the old one, a failure cannot leave the target truncated, and a filesystem that will not restrict the file is an error rather than a silent world-readable write. A file found with looser permissions is
tightened the next time it is read — installs made before this existed are not
left as they were, which for files rewritten this rarely could otherwise mean
forever. A file that is already private is left exactly as it is, including a
stricter mode an operator chose.

On Windows these files inherit the user profile ACL, which already restricts
them to the owner; there is no portable `chmod` equivalent applied.

### What this does not do

It does not defend against someone holding the disk. The device seed is still
plaintext, so a lost and unencrypted laptop is a compromised device — and on the
machine that created the user identity, a compromised *user* identity, because
the recovery phrase is beside it. Encrypting that material, or handing it to the
OS keychain, is a separate change; note that the daemon also runs headless,
where no keychain is available to prompt, so it cannot simply be required.

For revocation, see below.


## Distrusting An Identity

`enox member remove` evicts a peer. That is the right tool for a device you no
longer use, and the wrong one for a device someone else now has: whoever holds
the user root key can mint fresh devices, and removing them one peer ID at a
time never finishes.

`enox member distrust` disowns the *identity*:

```bash
enox member distrust 12D3KooWAbc... --reason "laptop stolen"
enox member trust 12D3KooWAbc...      # take it back
```

It takes a peer ID or a user public key; a peer ID is resolved to the identity
its owner claim proves. Every device proving that identity is refused admission
from then on, **including devices created afterwards** — which is the part
removal cannot do.

The record lives in the circle's control document as `distrusted_users`, keyed
by user public key, signed by an admin over `distrust:{user_pubkey_hex}`. The
signature covers the resolved key rather than what was typed, so a record can be
checked later against the key it actually names.

That signature is checked **wherever the record is enforced**, not where it
arrives. The control document is replicated and every member can write to it, so
an entry that merely exists proves nothing — without the check, any member could
add one and lock an arbitrary identity out of the circle. An unsigned or
wrongly-signed record is ignored, and so is any record in a circle with no admin
key on file, because there is nothing to check it against.

Both admission paths consult it: the automatic one and `enox member approve`.
Approving is the admin's decision, but so was the distrust, and a manual
approval that quietly skipped the check would leave the two disagreeing.

It is part of the durable snapshot, so it survives every device in the circle
being offline at once. A boundary that a restart forgets is not a boundary.

### Why the circle decides, not the root key

The obvious place to revoke an identity is the user root key that issued it. In
the case that motivates revocation — a lost or stolen device — the root key is
*on that device*, so whoever took it could revoke the rightful owner just as
easily. An admin of a circle can always speak for that circle, and a circle is
where the damage lands.

The cost is that distrust is per circle. An identity disowned in one circle is
untouched in another, and each has to act for itself.

### What it does not do yet

- **Devices already admitted stay** until removed. Distrust stops readmission;
  `enox member remove` evicts what is already inside, and the MLS epoch change
  is what actually cuts off content.
- **A peer that proves nothing cannot be caught.** Distrust matches on a proven
  identity, and nothing yet *requires* a joiner to prove one — so an attacker who
  simply publishes no owner claim is refused by the invite grant, not by this.
  Requiring proof to join is the step after this one.
- **It disowns the honest devices too.** That is inherent: if the identity is
  compromised, the circle is disowning the identity. Make a new one and be
  re-invited.
