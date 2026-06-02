# Identity & Membership Model (reconciled)

**Status:** design / to implement. This document reconciles two conflicting
descriptions in the existing docs and introduces a stable device/user identity
layer. It **supersedes** the epoch→PSK rotation described in
[`../security.md`](../security.md) §"MLS — RFC 9420 access revocation".

---

## 0. The contradiction we are resolving

Two docs disagree about what the PSK is:

- [`archived/admin.md`](archived/admin.md): *"PSK = transport filter (can you reach the swarm)"*,
  and explicitly *"PSK rotation is not required"* — eviction is the signed
  member list.
- [`security.md`](../security.md): PSK is **rotated on every MLS epoch**, and
  rotation is the eviction mechanism.

The implementation followed `security.md`. In practice that coupling is the root
cause of the "synced once, then permanently locked out" failures:

1. The transport PSK = the rolling MLS epoch key, so **connectivity depends on
   being at the exact current epoch.**
2. The MLS Welcome/Commit that lets a peer derive the new epoch key is delivered
   **over the transport gated by that same new key** → a peer one epoch behind
   can't connect to receive the key that would let it connect. Circular.
3. `rotate_psk_and_restart` tears down all connections on **every** membership
   change (adds included, not just removes), amplifying the race.
4. Eviction is already enforced independently by the `mls_removed` tombstone
   sync-gate ([`sync.rs`](../../src/network/sync.rs)), so the PSK rotation is
   **redundant** with an already-working layer.

**Decision:** the PSK is a **stable per-circle transport gate** (as `admin.md`
intended). It is **not** rotated per epoch. Eviction is enforced at the
membership layer (signed member list + tombstone sync-gate), and cryptographic
content-level eviction is a separate, optional future layer (§6).

---

## 1. Identity hierarchy

Three levels, replacing today's single per-circle ephemeral keypair:

```
User            (a person; links many devices)            handle: "suzy"
  └─ Device     (a machine/install; stable identity)      label:  "suzy-macbook"
       └─ Agent (a human or AI actor in a circle)         name:   "alice", "claude-reviewer"
```

| Level  | Key material | Scope | Stable across | Purpose |
|--------|--------------|-------|---------------|---------|
| **User**   | Ed25519 "user root key" | global, all devices | forever | links devices to one person; signs device attestations |
| **Device** | Ed25519 "device key" | global, all circles on this install | forever (survives restarts & re-joins) | the libp2p/MLS identity root; *this is what was being regenerated per-join before* |
| **Agent**  | none (a label) — or optional sub-key | per circle session | per session | attribution of presence & writes; multiple per device |

### 1.1 Device identity

- One Ed25519 **device key**, generated **once** on first startup, stored in
  `~/.enoxian/identity.toml` (global) — **not** per-circle.
- `device_id = multihash(device_pubkey)` — stable, unforgeable.
- Human-readable `device_label` (e.g. `suzy-macbook`), editable.
- **Per-circle connection keys are derived, not regenerated:**
  `circle_key = HKDF(device_key, "enoxian-circle/" || circle_id)`.
  - Deterministic ⇒ the same device always presents the **same** peer ID in a
    given circle, across restarts and re-joins. (Fixes the churn that forced MLS
    re-adds and epoch advances.)
  - Per-circle ⇒ an outside observer in two circles can't link the two peer IDs
    to one device. (Cross-circle unlinkability.)
  - Ownership is provable: the device can sign with `device_key` to prove a
    derived `circle_key` belongs to it.

> Replaces the current `keypair_proto_hex` written fresh into every circle's
> `config.toml` by `enox init`/`enter`.

### 1.2 User identity

- One Ed25519 **user root key**. `user_id = multihash(user_pubkey)`,
  human-readable `user_handle` (e.g. `suzy`).
- Links devices via **attestations**:
  `attestation = sign(user_key, device_pubkey || device_label || issued_at)`.
  A device stores its own attestation; it proves "this device belongs to user
  `suzy`" without exposing the user private key.
- Published in a circle's member list so peers can group device-peers under one
  user ("suzy — 3 devices online") and so an admin can add/remove a **user**
  (all their devices) rather than individual device peers.

**How the user key reaches a second device — _decided: attestation chain +
mnemonic backup._** Device 1 holds the user key and signs an attestation for
device 2's pubkey (transferred via QR / one-time link token). Device 2 stores
its own device key + the attestation, and **never holds the user private key**.
A BIP39-style mnemonic backup of the user key is generated at user-creation time
so losing device 1 isn't fatal (importing the mnemonic on any device
reconstitutes the user key / CA). The user root key acts like an SSH CA: it
signs device certs; devices present them.

### 1.3 Agents

- An **agent** is a named human or AI actor operating *through* a device in a
  circle: `alice`, `claude-reviewer`, etc. Multiple agents per device.
- **MLS membership and the libp2p peer identity live at the _device_ level.** A
  device joins a circle once (one connection, one MLS leaf). Agents are
  multiplexed over that single connection at the application layer:
  - presence/awareness entries are keyed by `(device_id, agent_name)`;
  - CRDT writes are attributed to the active agent;
  - this keeps MLS membership clean (one leaf per device, not per agent) while
    still showing every agent distinctly in the UI.
- Replaces today's `agent_id` (peer-id tail / `$ENOXIAN_AGENT_ID`) with an
  explicit, chosen name that defaults to the device label / user handle.

---

## 2. Reconciled security layers

| Layer | Mechanism | What it proves | Changed? |
|-------|-----------|----------------|----------|
| 1 Transport | **Stable per-circle PSK** (XSalsa20 / `pnet`) | you hold the circle's network secret | **was rotating; now stable** |
| 2 Identity | Noise + device per-circle key | you own this device identity | now **stable** across restarts/re-joins |
| 3 Membership | signed member list + `mls_removed` tombstone sync-gate | you're a current member, not evicted | unchanged — **this is the eviction boundary** |
| 4 Content *(future, optional)* | MLS group key encrypts CRDT updates | you can *read* current content | new; replaces the (mis)use of epoch→PSK |

- **MLS is kept** for member key agreement, group membership cryptography, and as
  the basis for Layer 4 — but its epoch key is **no longer the transport PSK**.
- `rotate_psk_and_restart` and the `epoch_psk → pnet` derivation
  ([`lifecycle.rs`](../../src/lifecycle.rs)) are removed from the connectivity
  path.

---

## 3. Eviction semantics (without PSK rotation)

`enox member remove <user|device>`:
1. Admin signs the removal in the member list and writes an `mls_removed`
   tombstone (same CRDT transaction) — for a user, tombstone all their devices.
2. The tombstone replicates to all members.
3. Every member's sync-gate ([`sync.rs`](../../src/network/sync.rs)) rejects the
   removed peer **before any CRDT data is exchanged**.

**What this guarantees:** a removed device can still *open a TCP connection*
(stable PSK), but is rejected at the sync gate and **reads no new data**.

**What it does not (yet) guarantee:** cryptographic confidentiality against a
removed member who races the tombstone, or who connects to a member that hasn't
received the tombstone. (Note: the old epoch→PSK rotation had the *same* gap — a
member who missed the Remove commit was on the old PSK too.) Closing this fully
is Layer 4 (§6).

---

## 4. Startup & connect flow

**First launch (no `~/.enoxian/identity.toml`):** prompt —
- **Create new identity** → generate user key + device key; ask for `user_handle`
  and `device_label`. This device becomes the first device of a new user.
- **Link this device to an existing user** → generate device key; obtain a user
  attestation (scan QR / paste link token from an existing device, or enter the
  passphrase under §1.2-b). Store device key + attestation.

**Every later launch:** load the device identity automatically. It is the
**default identity** for all `enox init` / `enox enter` — no per-circle keygen,
no churn.

**Creating / joining a circle:** the device's derived `circle_key` is used as the
peer identity; the member entry records `device_id`, `user_id`, `device_label`,
and the user attestation.

**Connecting as an agent:** an agent (a human opening the UI, or an AI process
attaching) may pass an `agent_name`; default = device label / user handle.
Multiple agents on one device appear as distinct presences over the one
connection.

---

## 5. Migration from the current model

- Existing circles store a per-circle `keypair_proto_hex`. On upgrade: keep
  honoring it as that circle's key (so existing membership/MLS leaves don't
  break), but generate the global device identity and, for **new** circles, use
  derived `circle_key`s.
- Existing `psk_hex` becomes the stable transport PSK (stop rotating it).
- `enox enter` for an already-joined circle refreshes config without minting a
  new identity (already implemented — see the refresh path in
  [`enter.rs`](../../src/commands/enter.rs)).

---

## 6. Layer 4 — content encryption (future, optional)

If/when cryptographic eviction beyond the sync-gate is required: encrypt each
CRDT update with the MLS group (epoch) key before it goes on the wire, and
decrypt on receipt. Then a removed member who connects still cannot *read* new
content, and connectivity stays decoupled from membership. This is the
*correct* place for MLS epoch keys — at the message layer (which is what MLS,
"Messaging Layer Security", is for) — not as the transport PSK.

---

## 7. Decisions (resolved)

These were the open forks; all are now decided and reflected above:

- **A. User-key bootstrap → attestation chain + mnemonic backup.** User key is a
  CA held by a device; it signs device attestations; mnemonic backs it up. (§1.2)
- **B. Connection keys → per-circle derived** (`HKDF(device_key, circle_id)`).
  Stable per circle (no churn), unlinkable across circles. (§1.1)
- **C. Membership granularity → track devices, manage users.** The member list
  stores individual devices; admin operations act on a user and expand to all
  their devices. (§3)
- **D. Agents → pure labels over the device connection.** One MLS leaf / peer per
  device; agents are named, attributed presences. No per-agent keys/leaves. (§1.3)

## 8. Implementation sketch (for the follow-up build)

Ordered so each step is independently shippable:

1. **Stop the rotation (unblocks sync now):** make `epoch_psk → pnet` rotation a
   no-op / remove `rotate_psk_and_restart` from the connectivity path; keep the
   `mls_removed` sync-gate as the eviction boundary. Update `security.md` body to
   match this banner.
2. **Device identity:** generate + persist a device key in
   `~/.enoxian/identity.toml`; derive per-circle keys via HKDF; have
   `init`/`enter` use the derived key instead of `generate_keypair()`.
3. **Startup flow:** first-run prompt (create vs link); auto-load thereafter;
   default identity for `init`/`enter`.
4. **User identity + attestations:** user key + mnemonic; QR/link-token device
   linking; publish attestations in the member list; group presence by user.
5. **Agents:** explicit agent names in presence/awareness keyed by
   `(device_id, agent_name)`; write attribution; UI to add/switch agents.
6. **(Optional, later) Layer 4 content encryption** (§6) if cryptographic
   eviction beyond the sync-gate is required.
