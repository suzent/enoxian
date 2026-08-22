# Security Model

This document describes the current security model. The implementation combines
stable device identity, a circle transport gate, authenticated peer sessions,
and MLS-derived content keys. The transport PSK is stable; it is not derived
again for every MLS epoch.

## Trust Boundaries

enoxian separates transport, identity, membership, and content:

| Layer | Mechanism | What it proves | Status |
|-------|-----------|----------------|--------|
| Transport | Stable per-circle PSK via `libp2p::pnet` | The peer holds the circle network secret | Implemented |
| Identity | Noise + per-circle Ed25519 key derived from the device key | The peer owns this device identity | Implemented |
| Membership | Signed member list + `mls_removed` tombstone sync gate | The peer has not been explicitly evicted | Implemented |
| Content | MLS exporter + HKDF + ChaCha20-Poly1305 | The peer holds the active MLS epoch secret and the frame was not modified | Implemented |

The public bootstrap server is outside the circle trust boundary. It provides
rendezvous and circuit relay only; it does not join any circle and does not hold
any circle PSK.

## Transport PSK

Every circle has a stable 256-bit pre-shared key. It is applied to direct TCP
circle-peer connections through `libp2p::pnet` before Noise starts. A peer with
the wrong PSK fails before sync protocol negotiation.

The PSK is a coarse network gate, not the revocation mechanism. It is distributed
in invite links and saved in `~/.enoxian/circles/<id>/config.toml`. It does not
expire after a peer joins, and it is not rotated on MLS epoch changes.

## Peer Identity

Each install has one stable device key in `~/.enoxian/identity.toml`.
Per-circle connection keypairs are derived deterministically from that device key
using HKDF-SHA256:

```text
HKDF(device_key, "enoxian-device-v1", "circle/<circle-id>")
```

The result is a stable peer ID for each `(device, circle)` pair. Noise proves
ownership of that per-circle key during connection setup.

Implications:

- A peer cannot impersonate another peer ID without the corresponding key.
- Rejoining the same circle from the same device presents the same peer ID.
- A removed peer that reconnects with the same identity can be rejected by the
  tombstone sync gate.

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
4. `src/network/sync.rs` checks `mls_removed` before exchanging any CRDT data
   and rejects tombstoned peers.

The MLS Remove commit advances the content-encryption epoch. A removed member
can process the removal but cannot export the new epoch secret, so it cannot
decrypt subsequent content frames. Tombstones remain a useful early sync gate;
the MLS epoch is the cryptographic boundary.

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
exporter secret is never used as the transport PSK.

## Content Frames

The v2 sync, proposal, and workspace-event protocols encrypt every logical
payload with ChaCha20-Poly1305. The frame header contains a fixed magic value,
format version, purpose, MLS epoch, and random nonce. The header and circle ID
are authenticated as associated data. HKDF-SHA256 domain-separates CRDT,
proposal, and event keys from the MLS exporter secret, preventing ciphertext
from being moved between protocol purposes or circles.

A separate `/enoxian/mls-bootstrap/1.0.0` stream solves the join/offline
bootstrap cycle. It carries only KeyPackages, signed owner/pending/member
records, targeted Welcomes, removal tombstones, and MLS commits. It is protected
by the stable circle PSK and Noise, but not by the content key because a joiner
does not have that key yet. It never carries workspace files, chat, tasks,
proposal content, event-log entries, or blobs.

## Current Attacker Capabilities

### Current Member With The PSK

| Action | Possible? |
|--------|-----------|
| Connect to the circle swarm | Yes |
| Read synced workspace files and chat | Yes |
| Write CRDT updates | Yes |
| Impersonate another peer ID | No, Noise proves key ownership |
| Perform admin member operations | No, requires `admin.key` |

### Removed Member Who Still Has The Stable PSK

| Action | Possible? |
|--------|-----------|
| Open a transport connection | Yes, if the PSK is still known |
| Complete a sync session with peers that have the tombstone | No |
| Read new CRDT/proposal/event content after the removal epoch | No |
| Read data already synced to local disk | Yes |
| Read bootstrap membership records and MLS commits | Yes, after completing the PSK + Noise transport |

### Outside Peer Without The PSK

| Action | Possible? |
|--------|-----------|
| Connect to direct PSK-TCP circle peers | No |
| Read circle content | No |
| Discover local peer IDs and addresses via mDNS | Yes, on the same LAN |

## Invites

Invite links contain the circle ID, stable PSK, expiry timestamp, optional circle
name, optional peer/relay/rendezvous addresses, and optional admin public key.

Expiry is enforced by `enox enter` before joining. It prevents accidental or
late use of old links, but it does not revoke a peer that already joined and
saved the PSK.

Practical guidance:

- Share invite links only over trusted channels.
- Use short TTLs for one-off onboarding.
- Remove unwanted members promptly so the tombstone propagates.
- Treat the PSK as a durable secret until explicit circle-key rotation is added.

## Relay And Rendezvous

The bootstrap server (`enox bootstrap serve`) is centralized network
infrastructure, not a centralized trust core. It learns metadata such as peer
IDs, circle UUID namespaces, timing, addresses, and traffic volume. It does not
hold the PSK and does not parse circle sync frames.

Data paths:

- LAN or static-IP direct path: TCP + PSK + Noise + Yamux.
- Bootstrap/rendezvous path: QUIC to the bootstrap server for discovery.
- Circuit relay fallback: Noise-protected relay circuit when direct dialing
  fails.

Relay traffic is opaque to the relay at both the libp2p transport layer and the
MLS-derived content layer. Authorized current members decrypt content locally.

## Residual Metadata Leakage

Content encryption protects payloads, not traffic shape. A relay or network
observer can still learn peer IDs used for routing, IP/address information available to the
transport, connection timing and duration, protocol selection, frame sizes,
frame counts, and traffic volume. The bootstrap stream additionally exposes
membership delivery records to a peer that still knows the circle PSK. MLS
epochs and nonces are visible in encrypted frame headers. File paths are inside
the encrypted CRDT frame and proposal/event metadata and blobs are encrypted as
one authenticated payload.

## Local Daemon API

The managed daemon also exposes a local HTTP/WebSocket API for the CLI and web UI. This API
is a control plane, not the WAN relay. It can read and mutate circle state, so it
must be treated as privileged local infrastructure.

The HTTP/WebSocket listener binds to loopback by default. Every API request
requires the bearer token stored at `~/.enoxian/api.token`, and CORS permits only
local origins. Explicit LAN binding widens the attack surface and should be used
only on a trusted network.

## Admin Key

The circle creator generates an Ed25519 admin keypair at `enox init`. The
private key (`admin.key`) remains on the creator's machine; the public key is
embedded in invites so peers can verify admin operations.

Only the holder of `admin.key` can add, remove, approve, reject, or promote
members. If the admin private key is compromised, an attacker can perform member
operations. Admin key rotation and multi-admin recovery are not yet implemented.

## LAN Exposure

mDNS announces peer IDs and listen addresses on the local network. It does not
expose circle content or the PSK. Peer IDs and addresses should nevertheless be
treated as metadata visible to other devices on the LAN.

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
