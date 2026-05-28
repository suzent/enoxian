# Security Model

## Layers of protection

ENOCHIAN uses two independent security layers:

| Layer | Mechanism | What it proves |
|-------|-----------|----------------|
| **Transport** | PSK (XSalsa20 via `pnet`) | You hold the circle secret |
| **Identity** | Noise + Ed25519 keypair | You own the private key behind this peer ID |

Both layers are applied on every connection before any data is exchanged.

---

## PSK — who can connect

Every circle has a 256-bit pre-shared key. It is applied at the TCP layer via `libp2p::pnet` before the Noise handshake. A peer with the wrong (or no) PSK fails immediately and silently — the XSalsa20 stream is garbled and the connection drops.

**The PSK is the primary access credential.** Anyone who holds it can connect to the circle swarm.

The PSK is distributed via invite links (`enochian://v1/...`). Once a peer has joined, the PSK lives in their `~/.enochian/circles/<id>/config.toml` indefinitely. There is no expiry.

---

## Peer identity — who you are

Each peer has an Ed25519 keypair generated at `enoch enter` time. The peer ID is a hash of the public key. The Noise protocol proves key ownership on every connection: you cannot claim a peer ID without the corresponding private key.

**Peer IDs are unforgeable but not tied to a person.** Anyone with the PSK can generate a fresh keypair and join as a new identity.

Implications:
- Impersonating a specific existing peer (faking their peer ID) is cryptographically impossible
- Rejoining with a new peer ID after being removed requires only the PSK, which the removed member still holds

---

## Member list and MLS group — what is enforced

The member list (`member_list` Y.Map in the control doc) is a CRDT-replicated directory of peer IDs and roles. Write operations (add/remove/promote) require an admin signature. This is an auditable log of who has ever been in the circle, but it is not the security boundary on its own.

The security boundary is the **MLS group** + **PSK**.

Behaviour on `enoch member remove <peer>`:

1. Admin issues an MLS `Remove` + `Commit` for the peer's leaf node
2. The commit is broadcast via the `mls_commits` CRDT array to all remaining members
3. Each remaining member applies the commit, advancing their MLS epoch
4. A new PSK is derived from the new epoch key material using `export_secret("enochian-psk", ...)`
5. The admin immediately rotates the circle's pnet PSK to the new value and restarts their swarm
6. Remaining members see the commit and do the same via the commit-watcher observer
7. The removed peer's MLS state is at the old epoch — they cannot derive the new PSK and fail the pnet handshake on reconnect
8. The peer's entry is removed from the CRDT member list; any pending/welcome/key-package entries are also cleaned up
9. The peer's presence entry is written as Offline

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

Invite links have a TTL (`--ttl`, default 7 days). After expiry, `enoch enter` rejects the link.

**What invite TTL protects:** prevents a removed member from using a stale invite link to obtain a KeyPackage from the admin. It does **not** help if the member already joined — they have the PSK on disk.

**What MLS revocation protects:** when a member is removed (`enoch member remove`), the MLS Remove commit advances the epoch for all remaining members. The admin and each member derive a new PSK and restart their swarm. The removed peer's old PSK is immediately useless for connecting.

**Residual data:** the removed peer retains a local copy of everything they synced before removal. MLS provides forward secrecy (they cannot decrypt future traffic) but not backward secrecy (they keep old data). This is the standard MLS threat model.

**Practical guidance:** use short-lived invites (`--ttl 24h`) for any invite you might regret. Remove unwanted members promptly — the MLS epoch advance immediately locks them out of future sync.

---

## MLS — RFC 9420 access revocation (implemented)

ENOCHIAN uses **IETF MLS (RFC 9420)** for group key management, implemented via the [`openmls`](https://github.com/openmls/openmls) crate.

### How TreeKEM works

Members are arranged as leaf nodes of a binary Merkle-style tree. Each leaf holds the member's public key. Every path from a leaf to the root has a corresponding chained secret. When a member is added or removed, only the nodes on the affected paths are re-keyed — the `O(log N)` update is efficient even for large groups.

On **Remove**: the removed leaf's node is blanked. A new epoch root secret is derived that depends only on the remaining members' key material. The evicted peer has no path to the new root secret, regardless of whether they rejoin with the same or a different keypair.

### The MLS epoch → PSK derivation chain

```
MLS epoch secret
    └─ export_secret("enochian-psk", [], 32)
         └─ pnet PSK (XSalsa20 stream cipher key)
```

Every MLS epoch produces a distinct 32-byte PSK. Remaining members all derive the same PSK (they are in the same epoch). The evicted peer is at the old epoch and cannot derive the new PSK — their pnet handshake fails immediately.

### Commit propagation

MLS commits are broadcast via the `mls_commits` Y.Array in the control CRDT doc. Each commit carries:
- `epoch` — the epoch this commit advances to
- `data_hex` — TLS-serialised `MlsMessageOut`
- `sender_peer_id` — who issued the commit
- `ratchet_tree_hex` — ratchet tree extension for Add commits (empty for Remove)

A serial commit-watcher task in each peer's daemon applies commits in order and rotates the PSK after each epoch advance. Serialisation via an `mpsc` channel prevents races when multiple commits arrive in one P2P sync batch.

### Offline members

Offline members who miss the Remove commit will receive it when they next connect — the CRDT array is replicated on reconnect. They apply it, derive the new PSK, and restart their swarm. The evicted peer cannot interfere: they fail the pnet handshake before any CRDT sync occurs.

### What MLS does NOT protect

- **Past data on disk:** MLS provides *forward* secrecy. The removed member keeps a local copy of everything they received before removal.
- **Colluding members:** any current member can share the PSK or MLS epoch secret out-of-band. MLS models honest-but-curious adversaries who follow the protocol.
- **Admin key compromise:** if the admin's Ed25519 key is stolen, an attacker can approve their own KeyPackage. Admin key rotation is not yet implemented.

---

## Admin keypair

The circle creator generates an Ed25519 admin keypair at `enoch init`. The private key (`admin.key`) never leaves the creator's machine. The public key is embedded in invite URIs so all peers can verify admin signatures.

Only the holder of `admin.key` can:
- Add or remove members (including issuing MLS Add/Remove commits)
- Approve or reject join requests
- Promote a member to admin

There is currently no mechanism to transfer admin authority to another peer (multi-admin is not implemented). Losing `admin.key` means member operations can no longer be performed for that circle, and the MLS group cannot be updated.

The daemon auto-signs on behalf of the admin when the frontend omits a signature — it reads `admin.key` from disk and signs locally. This means the admin machine's daemon is privileged: anyone with local access to `~/.enochian/circles/<id>/admin.key` can perform admin operations.

---

## LAN exposure

mDNS peer discovery broadcasts peer IDs and listen addresses on the local network. This reveals:
- That an ENOCHIAN daemon is running
- The peer's libp2p peer ID
- The TCP port the daemon is listening on

No circle content or PSK is exposed via mDNS. Connection attempts from peers with the wrong PSK are dropped silently. On networks where this exposure is unacceptable, disable mDNS (planned flag: `--no-mdns`) and rely on explicit invite peer addresses or the WAN anchor node.
