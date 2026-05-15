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

## Member list — what it currently enforces

The member list (`member_list` Y.Map in the control doc) is a CRDT-replicated directory of peer IDs and roles. It is enforced at the API layer for write operations (add/remove/promote require an admin signature), but it is **not enforced at the connection or sync level**.

Current behaviour on `enoch member remove`:
- The peer's entry is deleted from the member list CRDT
- The deletion replicates to all online peers
- The removed peer continues to receive sync updates; their connection is not dropped
- On daemon restart, they do not re-register themselves (the CRDT key is gone and auto-registration skips existing entries)
- If they rejoin with a new keypair (same or different machine), they appear as a new peer and are auto-registered again

In short: **member removal is a directory operation, not access revocation**.

---

## What an attacker can and cannot do

### With the PSK

| Action | Possible? |
|--------|-----------|
| Connect to the circle swarm | ✓ Yes |
| Read all synced workspace files | ✓ Yes |
| Read all chat history | ✓ Yes |
| Impersonate a specific peer ID | ✗ No — Noise proves key ownership |
| Sign member operations (add/remove/promote) | ✗ No — requires `admin.key` |
| Inject arbitrary writes into the CRDT | ✓ Yes — all peers with the PSK can write |

### Without the PSK

| Action | Possible? |
|--------|-----------|
| Connect to the circle swarm | ✗ No — PSK handshake fails immediately |
| Read any data | ✗ No |
| Discover that a circle exists via mDNS | ✓ Yes — mDNS is unencrypted; peer IDs and addresses are visible on LAN |

---

## Invite TTL vs. PSK revocation

Invite links have a TTL (`--ttl`, default 7 days). After expiry, `enoch enter` rejects the link. A removed member has two paths back into the circle:

**Path 1 — via invite:** `enoch enter <old-invite>` decodes the link, extracts the PSK, generates a fresh keypair, and joins as a new peer ID. This path is **blocked** once the invite expires. Short-lived invites (hours rather than days) close this window quickly.

**Path 2 — via existing config:** after joining, the PSK is saved to `~/.enochian/circles/<id>/config.toml`. A removed member can craft a new config entry with a fresh keypair but the same PSK and start `enochd` directly — no invite needed. Invite TTL has no effect on this path.

Path 2 requires knowing where the config lives and how to write a new keypair entry. It is not a one-liner, but it is not hard for a technical user. The only way to close both paths is PSK rotation.

**Practical guidance:** use short-lived invites (e.g. `--ttl 24h`) for anyone you might need to remove. That limits the path-1 window to hours. Accept that path 2 exists until PSK rotation is implemented (M11).

---

## Planned: true access revocation (MLS — RFC 9420)

Blocking a peer ID alone is insufficient — the removed peer can rejoin with a fresh keypair. Custom PSK rotation is also fragile (coordination window, restart requirement). The correct solution is **IETF MLS (RFC 9420)**, the international standard for group key management in decentralised systems.

**How MLS solves this (M11):**

MLS uses TreeKEM — members are arranged in a binary tree. When a member is evicted, their node is pruned and a new epoch root key is derived from the remaining members' public keys. The evicted peer's key material is cryptographically useless for all future epochs.

1. Admin runs `enoch member remove <peer>` — issues an MLS `Remove` + `Commit`
2. A new epoch key is derived; remaining online peers receive it immediately
3. Offline members receive pending commits via KeyPackages when they next reconnect — no coordination window required
4. The removed peer cannot decrypt any future CRDT sync data regardless of whether they reconnect with the same or a new keypair, because they have no valid KeyPackage for the new epoch
5. The existing PSK (transport layer) remains as a coarse admission gate; MLS provides the fine-grained revocation

The Rust implementation is [`openmls`](https://github.com/openmls/openmls), a production crate implementing RFC 9420. See M11 in the roadmap for the full integration plan.

---

## Admin keypair

The circle creator generates an Ed25519 admin keypair at `enoch init`. The private key (`admin.key`) never leaves the creator's machine. The public key is embedded in invite URIs so all peers can verify admin signatures.

Only the holder of `admin.key` can:
- Add or remove members
- Promote a member to admin
- (Planned) Rotate the PSK

There is currently no mechanism to transfer admin authority to another peer (multi-admin is not implemented). Losing `admin.key` means member operations can no longer be performed for that circle.

---

## LAN exposure

mDNS peer discovery broadcasts peer IDs and listen addresses on the local network. This reveals:
- That an ENOCHIAN daemon is running
- The peer's libp2p peer ID
- The TCP port the daemon is listening on

No circle content or PSK is exposed via mDNS. Connection attempts from peers with the wrong PSK are dropped silently. On networks where this exposure is unacceptable, disable mDNS (planned flag: `--no-mdns`) and rely on explicit invite peer addresses or the WAN anchor node.
