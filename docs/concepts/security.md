# Security Model

This document describes the current security model. Older notes that mention
deriving the transport PSK from each MLS epoch are obsolete; the authoritative
identity rationale lives in [plan/identity.md](../plan/identity.md).

## Trust Boundaries

enoxian separates transport, identity, membership, and content:

| Layer | Mechanism | What it proves | Status |
|-------|-----------|----------------|--------|
| Transport | Stable per-circle PSK via `libp2p::pnet` | The peer holds the circle network secret | Implemented |
| Identity | Noise + per-circle Ed25519 key derived from the device key | The peer owns this device identity | Implemented |
| Membership | Signed member list + `mls_removed` tombstone sync gate | The peer has not been explicitly evicted | Implemented |
| Content | MLS epoch key encrypts CRDT updates | The peer can read current content | Planned |

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

This blocks new sync sessions from removed peers. It does not yet provide
cryptographic content secrecy against a removed peer racing a member that has
not received the tombstone. That stronger guarantee is the planned content
encryption layer.

## MLS

enoxian uses IETF MLS (RFC 9420), implemented with `openmls`, for group
membership cryptography and future content-layer encryption.

MLS commits are replicated in the control doc:

- `epoch` — the epoch this commit advances from
- `data_hex` — TLS-serialized `MlsMessageOut`
- `sender_peer_id` — who issued the commit
- `ratchet_tree_hex` — ratchet tree extension data for joins

Each daemon applies incoming commits serially to avoid races. The MLS epoch key
is tracked for future content encryption; it is not derived into the transport
PSK.

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
| Read new CRDT updates from tombstone-aware peers | No |
| Read data already synced to local disk | Yes |
| Read future encrypted content after Layer 4 ships | No, if they lack the current MLS content key |

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

The bootstrap server (`enoxd --bootstrap`) is centralized network
infrastructure, not a centralized trust core. It learns metadata such as peer
IDs, circle UUID namespaces, timing, addresses, and traffic volume. It does not
hold the PSK and does not parse circle sync frames.

Data paths:

- LAN or static-IP direct path: TCP + PSK + Noise + Yamux.
- Bootstrap/rendezvous path: QUIC to the bootstrap server for discovery.
- Circuit relay fallback: Noise-protected relay circuit when direct dialing
  fails.

Relay traffic is opaque to the relay at the libp2p transport layer, but current
circle members still receive plaintext CRDT updates after decrypting their peer
connection. Full content-layer E2EE is planned separately.

## Local Daemon API

`enoxd` also exposes a local HTTP/WebSocket API for the CLI and web UI. This API
is a control plane, not the WAN relay. It can read and mutate circle state, so it
must be treated as privileged local infrastructure.

Current hardening target:

- Default the HTTP/WS listener to loopback.
- Restrict CORS.
- Add local API authentication for browser and CLI clients.

## Admin Key

The circle creator generates an Ed25519 admin keypair at `enox init`. The
private key (`admin.key`) remains on the creator's machine; the public key is
embedded in invites so peers can verify admin operations.

Only the holder of `admin.key` can add, remove, approve, reject, or promote
members. If the admin private key is compromised, an attacker can perform member
operations. Admin key rotation and multi-admin recovery are not yet implemented.

## LAN Exposure

mDNS announces peer IDs and listen addresses on the local network. It does not
expose circle content or the PSK. On networks where peer discovery metadata is
sensitive, use explicit peer/rendezvous addresses and disable mDNS once the
planned flag exists.
