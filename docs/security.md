# Security Model

> **⚠️ Reconciliation note (supersedes part of this doc).** The
> epoch→PSK **rotation** described below (§"MLS — RFC 9420 access revocation"
> and Layer 1/4 of the table) is **superseded** by
> [`plan/identity.md`](plan/identity.md). The transport PSK is now a **stable
> per-circle network gate** (matching [`plan/admin.md`](plan/admin.md): *"PSK
> rotation is not required"*); eviction is enforced by the signed member list +
> `mls_removed` tombstone sync-gate (Layer 3), with optional MLS **content**
> encryption as the future cryptographic-eviction layer. Rotating the transport
> PSK per epoch coupled connectivity to epoch-sync and caused permanent
> lock-outs (a peer one epoch behind can't connect to receive the key that would
> let it connect). The sections below describe the *previous* model; treat
> `plan/identity.md` as authoritative.

## Layers of protection

enoxian applies three independent security layers in sequence. Each layer rejects peers that fail its check before passing control to the next.

| Layer | Mechanism | What it proves |
|-------|-----------|----------------|
| **Transport** | Stable PSK (XSalsa20 via `pnet`) | You hold the circle's network secret |
| **Identity** | Noise + per-circle Ed25519 keypair (derived from device key) | You own this device identity |
| **Sync gate** | `mls_removed` tombstone + signed member list | You have not been explicitly evicted |

Each layer is independent. Failing any layer drops the peer before the next is reached.

> **Note:** A fourth layer (MLS epoch key encrypting CRDT content) is planned — see
> [`plan/identity.md`](plan/identity.md) §6. When implemented, it will provide
> cryptographic eviction: a removed member can connect but cannot decrypt new updates.

---

## PSK — who can connect

Every circle has a stable 256-bit pre-shared key. It is applied at the TCP layer via `libp2p::pnet` before the Noise handshake. A peer with the wrong (or no) PSK fails immediately and silently — the XSalsa20 stream is garbled and the connection drops.

**The PSK is the primary access credential.** Anyone who holds it can connect to the circle swarm. **It does not rotate** — it is a stable per-circle network gate. Eviction is handled at Layer 3 (the sync gate), not by re-keying the transport.

The PSK is distributed via invite links (`enoxian://v1/...`). Once a peer has joined, the PSK lives in their `~/.enoxian/circles/<id>/config.toml` indefinitely. There is no expiry.

---

## Peer identity — who you are

Each device has one stable Ed25519 **device key**, stored in `~/.enoxian/identity.toml`. Per-circle connection keypairs are derived deterministically from the device key via HKDF-SHA256: `HKDF(device_key, "enoxian-device-v1", "circle/<id>")`. The peer ID is a hash of the derived public key.

**This means the peer ID for a given (device, circle) pair is stable across daemon restarts and re-joins.** The Noise protocol proves key ownership on every connection.

Implications:
- Impersonating a specific existing peer is cryptographically impossible
- The same device always presents the same peer ID in a given circle — no MLS re-add churn on restart
- Rejoining after removal still uses the same peer ID, which is now rejected at the sync gate (Layer 3)

---

## Member list and MLS group — what is enforced

The member list (`member_list` Y.Map in the control doc) is a CRDT-replicated directory of peer IDs and roles. Write operations (add/remove/promote) require an admin signature. This is an auditable log of who has ever been in the circle, but it is not the security boundary on its own.

The security boundary is the **MLS group** + **PSK**.

Behaviour on `enox member remove <peer>`:

1. Admin issues an MLS `Remove` + `Commit` for the peer's leaf node
2. The peer's entry is removed from the CRDT member list; a tombstone entry is written to the `mls_removed` CRDT map — **atomically in the same CRDT transaction**
3. The commit is broadcast via the `mls_commits` CRDT array to all remaining members
4. Each remaining member applies the commit, advancing their local MLS epoch (no transport restart)
5. Any pending/welcome/key-package entries are cleaned up; the peer's presence entry is written as Offline

**Eviction is enforced by the tombstone sync-gate:** `src/network/sync.rs` checks `mls_removed` at the top of every sync session — before any CRDT data is exchanged — and rejects tombstoned peers immediately. The tombstone propagates to all peers as part of the same CRDT update that removes the member entry, so there is no gap.

---

## What an attacker can and cannot do

### With the current PSK (a legitimate member)

| Action | Possible? |
|--------|-----------|
| Connect to the circle swarm | ✓ Yes |
| Read all synced workspace files | ✓ Yes |
| Read all chat history | ✓ Yes |
| Impersonate a specific peer ID | ✗ No — Noise proves key ownership |
| Sign member operations (add/remove/promote) | ✗ No — requires `admin.key` |
| Inject arbitrary writes into the CRDT | ✓ Yes — all members with the PSK can write |

### With a **stale/revoked** PSK (after being removed)

| Action | Possible? |
|--------|-----------|
| Connect to the circle swarm | ✗ No — pnet PSK has been rotated; XSalsa20 stream is garbled |
| Derive the new PSK from the old MLS epoch | ✗ No — TreeKEM ensures forward secrecy; new epoch key excludes removed leaves |
| Read new CRDT updates | ✗ No — transport rejected before any data |
| Read data synced before removal | ✓ Yes — data already on disk is not wiped |

### Without the PSK

| Action | Possible? |
|--------|-----------|
| Connect to the circle swarm | ✗ No — PSK handshake fails immediately |
| Read any data | ✗ No |
| Discover that a circle exists via mDNS | ✓ Yes — mDNS is unencrypted; peer IDs and addresses are visible on LAN |

---

## Invite TTL and PSK revocation

Invite links have a TTL (`--ttl`, default 7 days). After expiry, `enox enter` rejects the link.

**What invite TTL protects:** prevents a removed member from using a stale invite link to obtain a KeyPackage from the admin. It does **not** help if the member already joined — they have the PSK on disk.

**What MLS revocation protects:** when a member is removed (`enox member remove`), the MLS Remove commit advances the epoch for all remaining members. The admin and each member derive a new PSK and restart their swarm. The removed peer's old PSK is immediately useless for connecting.

**Residual data:** the removed peer retains a local copy of everything they synced before removal. MLS provides forward secrecy (they cannot decrypt future traffic) but not backward secrecy (they keep old data). This is the standard MLS threat model.

**Practical guidance:** use short-lived invites (`--ttl 24h`) for any invite you might regret. Remove unwanted members promptly — the MLS epoch advance immediately locks them out of future sync.

---

## MLS — RFC 9420 membership management (implemented)

enoxian uses **IETF MLS (RFC 9420)** for group key management, implemented via the [`openmls`](https://github.com/openmls/openmls) crate.

### How TreeKEM works

Members are arranged as leaf nodes of a binary Merkle-style tree. Each leaf holds the member's public key. Every path from a leaf to the root has a corresponding chained secret. When a member is added or removed, only the nodes on the affected paths are re-keyed — the `O(log N)` update is efficient even for large groups.

On **Remove**: the removed leaf's node is blanked. A new epoch root secret is derived that depends only on the remaining members' key material. The evicted peer has no path to the new root secret, regardless of whether they rejoin with the same or a different keypair.

### MLS epoch and eviction

MLS epochs advance on every membership change (Add / Remove). The epoch state is tracked locally for each member for future content-layer encryption (Layer 4). **The epoch key is no longer derived into the transport PSK** — that coupling caused lock-out races (see [`plan/identity.md`](plan/identity.md) for the full analysis). Eviction is instead enforced by the sync gate (Layer 3): the `mls_removed` tombstone rejects a removed peer before any CRDT data is exchanged.

### Commit propagation

MLS commits are broadcast via the `mls_commits` Y.Array in the control CRDT doc. Each commit carries:
- `epoch` — the epoch this commit advances to
- `data_hex` — TLS-serialised `MlsMessageOut`
- `sender_peer_id` — who issued the commit
- `ratchet_tree_hex` — ratchet tree extension for Add commits (empty for Remove)

A serial commit-watcher task in each peer's daemon applies commits in order and rotates the PSK after each epoch advance. Serialisation via an `mpsc` channel prevents races when multiple commits arrive in one P2P sync batch.

### Offline members

Offline members who miss a Remove commit receive it when they next connect — the CRDT array is replicated on reconnect. They apply it, and their local MLS epoch advances. The evicted peer is blocked at the sync gate regardless of when they reconnect.

### What MLS does NOT protect

- **Past data on disk:** MLS provides *forward* secrecy. The removed member keeps a local copy of everything they received before removal.
- **Colluding members:** any current member can share the PSK or MLS epoch secret out-of-band. MLS models honest-but-curious adversaries who follow the protocol.
- **Admin key compromise:** if the admin's Ed25519 key is stolen, an attacker can approve their own KeyPackage. Admin key rotation is not yet implemented.

---

## Admin keypair

The circle creator generates an Ed25519 admin keypair at `enox init`. The private key (`admin.key`) never leaves the creator's machine. The public key is embedded in invite URIs so all peers can verify admin signatures.

Only the holder of `admin.key` can:
- Add or remove members (including issuing MLS Add/Remove commits)
- Approve or reject join requests
- Promote a member to admin

There is currently no mechanism to transfer admin authority to another peer (multi-admin is not implemented). Losing `admin.key` means member operations can no longer be performed for that circle, and the MLS group cannot be updated.

The daemon auto-signs on behalf of the admin when the frontend omits a signature — it reads `admin.key` from disk and signs locally. This means the admin machine's daemon is privileged: anyone with local access to `~/.enoxian/circles/<id>/admin.key` can perform admin operations.

---

## LAN exposure

mDNS peer discovery broadcasts peer IDs and listen addresses on the local network. This reveals:
- That an enoxian daemon is running
- The peer's libp2p peer ID
- The TCP port the daemon is listening on

No circle content or PSK is exposed via mDNS. Connection attempts from peers with the wrong PSK are dropped silently. On networks where this exposure is unacceptable, disable mDNS (planned flag: `--no-mdns`) and rely on explicit invite peer addresses or the WAN anchor node.
