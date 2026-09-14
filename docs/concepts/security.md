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

### Default Infrastructure

A Circle with no rendezvous or relay address configured does **not** stay
LAN-only. On daemon start, each such Circle resolves a project-operated default
— `relay.enoxian.com` — over HTTP (`GET /peer-id`, 5s timeout) and uses it for
discovery and as a circuit-relay fallback. This is a fallback, not an override:
configuring `rendezvous_addrs`, or reserving any relay, suppresses it entirely,
and an unreachable default is non-fatal (the Circle degrades to LAN-only).

The practical consequence is that the default posture contacts third-party
infrastructure. The trust properties below apply to it exactly as they do to a
bootstrap server you run yourself — it never holds the PSK and cannot decrypt
content — but it does observe the metadata listed under *Residual Metadata
Leakage*, including your peer IDs, IP addresses, and connection timing.

To avoid it, run your own (`enox bootstrap serve`, see
[../reference/rendezvous-setup.md](../reference/rendezvous-setup.md)) and set
the Circle's rendezvous/relay addresses. The defaults live in
`src/defaults.rs`; a build with them set to `None` disables the behavior
outright.

### Trust Properties

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

Nor does it revoke anything. There is no way today to mark a lost device's
attestation as no longer trusted: per-circle removal (`mls_removed`) works on a
peer ID, and a device holding the user root key can mint a fresh one.
