# enoxian — Intuitive Overview

> A P2P workspace where AI agents and humans share files, tasks, and cryptographic membership.

---

## The Core Idea

Multiple agents — a human, Claude Code, a Suzent AI, another human on another machine — all work inside the **same directory** in real time. Files sync automatically. Tasks are shared. Who has which file open is visible. And when someone leaves the group, they are cryptographically locked out, not just removed from a list.

The unit of collaboration is called a **Circle**.

```
  Alice's laptop          Bob's desktop           Alice's suzent agent
  ┌─────────────┐         ┌─────────────┐         ┌─────────────┐
  │  enoxd     │◄───────►│  enoxd     │◄───────►│  enoxd     │
  │  ~/project/ │  P2P    │  ~/project/ │  P2P    │  ~/project/ │
  │             │  mesh   │             │  mesh   │             │
  │  enox CLI  │         │  enox CLI  │         │  API calls  │
  └─────────────┘         └─────────────┘         └─────────────┘
        ▲                        ▲                        ▲
        │ HTTP                   │ HTTP                   │ HTTP
        ▼                        ▼                        ▼
  ┌──────────┐             ┌──────────┐             ┌──────────┐
  │  human   │             │  human   │             │  suzent  │
  │  alice   │             │  bob     │             │  agent   │
  └──────────┘             └──────────┘             └──────────┘
```

Every participant runs one `enoxd` daemon per workspace. The daemon is the P2P node, the file syncer, and the HTTP server all in one. The `enox` CLI and any AI agent talk to it over localhost HTTP.

---

## Two Binaries

```
enoxd   long-running daemon
         ├── P2P swarm (libp2p)    — connects to other daemons
         ├── File watcher (notify) — disk ↔ CRDT sync
         └── HTTP server (axum)    — serves enox CLI + AI agents

enox    short-lived CLI
         └── reqwest HTTP calls    → enoxd on localhost:36521
```

A human types `enox status`. The CLI calls `GET /circles/{id}/api/status` on the local daemon and prints the result. That's the entire interface. AI agents do the same thing via HTTP calls.

---

## Identity: Three Layers

Every participant has three identifiers that mean different things:

```
owner        "alice"
             │  the human — groups all of alice's machines together
             │  set with --owner at join time, signed into membership
             │
             ├── peer_id   "12D3KooW..."
             │             the machine/daemon — unique libp2p keypair
             │             one per enoxd instance, persists across restarts
             │             this is what the MLS group tracks
             │
             │   agent_id  "alice"          ← human CLI on this machine
             │   agent_id  "alice-suzent"   ← suzent agent on same machine
             │   agent_id  "claude-code"    ← claude on same machine
             │
             │   (multiple agents share one peer_id — they all connect
             │    to the same enoxd daemon via localhost HTTP)
             │
             └── peer_id   "QmXyz..."
                           alice's second machine (desktop)
                           same owner, different peer_id
```

**Why three layers?**

- `owner` is for humans to identify who owns a machine
- `peer_id` is for the network and cryptography to track a specific daemon
- `agent_id` is for attribution — who wrote this file, who sent this message, who holds this lock

Chat and presence use `agent_id`. MLS group membership uses `peer_id`. The member list links them together via `owner`.

---

## A Circle's Lifetime

### Step 1 — Create

```
admin$ enox init --name "my-project" --owner alice

  Generates:
    circle_id   = random UUID
    PSK         = 32 random bytes  (the network password)
    peer keypair = Ed25519          (alice's machine identity)
    admin keypair = Ed25519          (signs member operations)
    MLS group   = single-member group (alice is leaf 0)

  Saves to ~/.enoxian/circles/<id>/
    config.toml   (circle_id, PSK, keypair, join_policy, owner)
    admin.key     (admin private key — creator only, never shared)
    mls/identity.json
    mls/group.json
```

### Step 2 — Invite

```
admin$ enox invite "my-project" --ttl 7d

  Returns: enoxian://ABC123...

  Encoded inside the URI:
    circle_id, circle_name
    PSK bytes
    admin public key
    relay/rendezvous addresses
    expiry timestamp
```

### Step 3 — Enter (Bob's machine)

```
bob$ enox enter enoxian://ABC123... --owner bob

  Bob's machine:
    generates its own peer keypair
    generates its own MLS identity + key package
    saves config (circle_id, PSK, keypair, owner=bob)

  Bob's daemon on first start:
    publishes key package    → mls_key_packages[bob_peer_id]
    signs owner claim        → mls_owner_claims[bob_peer_id]
                               sign(bob_peer_key, "owner:bob")
    writes pending entry     → mls_pending[bob_peer_id]
    connects to network via PSK
```

### Step 4 — Approve (admin's daemon)

```
  join_policy = "auto"   → admin daemon sees new mls_pending entry,
                           auto-approves immediately

  join_policy = "manual" → admin sees it in enox member pending,
                           runs enox member approve <peer_id>

  Either way, approve does:
    1. reads key package from mls_key_packages[bob_peer_id]
    2. runs MLS add_member()
         → commit bytes    (everyone else applies this)
         → welcome bytes   (only bob consumes this)
         → ratchet tree
    3. stores welcome    → mls_welcomes[bob_peer_id]
    4. stores commit     → mls_commits[]
    5. signs MemberEntry → member_list[bob_peer_id]
         signature covers "add:{peer_id}:{role}:owner:{owner}"
    6. removes from mls_pending
    7. saves MLS group state to disk
```

### Step 5 — Bob Joins the MLS Group

```
  Bob's daemon sees mls_welcomes[bob_peer_id]:
    calls MlsGroupManager::join_from_welcome()
    now has the same group epoch secrets as everyone else
    calls epoch_psk() → 32-byte key from MLS exporter
    this becomes the new pnet PSK for all connections
```

### Step 6 — Remove (epoch rotation locks out the removed peer)

```
admin$ enox member remove <bob_peer_id>

  1. MLS remove_member(bob_leaf_index)
       → new epoch, new epoch secrets
  2. epoch_psk() → completely new 32-byte PSK
  3. all remaining members apply the Commit
  4. all daemons reconnect with the new PSK
  5. Bob's daemon can no longer connect — it has the old PSK
```

The key point: **the PSK is derived from the MLS group epoch secret**, not stored statically. Removing a member changes the epoch, which changes the PSK. Bob is locked out at the transport layer — he can't even reach the network, let alone read files.

---

## The MLS Security Layer

MLS (RFC 9420) is a group key agreement protocol. Think of it as a cryptographic ratchet where every membership change advances the state.

```
  Epoch 0: [alice]
  │  alice creates the group, epoch_psk = derive("enoxian-psk", epoch_0_secret)
  │
  ├─ add bob ──────────────────────────────────────────── Epoch 1: [alice, bob]
  │  alice runs add_members(bob_key_package)               epoch_psk = derive(..., epoch_1_secret)
  │  → commit (everyone applies)                           PSK rotates — all reconnect
  │  → welcome (only bob consumes to join)
  │
  ├─ add carol ────────────────────────────────────────── Epoch 2: [alice, bob, carol]
  │  epoch_psk rotates again
  │
  └─ remove bob ───────────────────────────────────────── Epoch 3: [alice, carol]
     epoch_psk = derive(..., epoch_3_secret)
     bob does not have epoch_3_secret
     bob cannot derive the new PSK
     bob is locked out at the network layer
```

Each epoch's PSK is derived via:
```
group.export_secret(crypto, "enoxian-psk", &[], 32)
```

This is a one-way derivation — knowing an old PSK tells you nothing about the current one.

---

## The Network Layer

Peers connect to each other over libp2p with a layered transport:

```
  ┌──────────────────────────────────────────────────────────────────┐
  │                     Circle Network                               │
  │                                                                  │
  │   alice-laptop ◄──── TCP + PSK + Noise + Yamux ────► bob-desktop │
  │                                                                  │
  │   PSK = epoch_psk() derived from MLS group                       │
  │   Noise = authenticated encryption (peer identity)               │
  │   Yamux = multiplexed streams                                    │
  └──────────────────────────────────────────────────────────────────┘
         │                              │
         │ (can't reach directly?)      │
         ▼                              ▼
  ┌──────────────────────────────────────────────────────┐
  │                   Bootstrap Server                   │
  │              enoxd --bootstrap (public VPS)         │
  │                                                      │
  │   QUIC only (no PSK) — not a circle member           │
  │   Rendezvous: peers register & discover each other   │
  │   Relay: proxies connections for NAT-blocked peers   │
  └──────────────────────────────────────────────────────┘
```

Discovery order: mDNS (same LAN, instant) → rendezvous (WAN, via bootstrap server) → relay (NAT traversal fallback via bootstrap server).

---

## The Data Layer

Inside a circle, all state lives in **Yjs CRDTs** — conflict-free data structures that merge correctly no matter what order updates arrive.

```
  ┌─────────────────────────────────────────────────────┐
  │                    AppState                         │
  │                                                     │
  │  File docs (one per file in workspace)              │
  │  ┌─────────────────────────────────────────────┐    │
  │  │  "src/main.rs"  →  Y.Text  ←──────────┐     │    │
  │  │  "README.md"    →  Y.Text  ←──────────┐     │    │
  │  │  "notes.txt"    →  Y.Text  ←──────────┐     │    │
  │  └─────────────────────────────────────────────┘    │
  │           ▲                              │          │
  │    file   │ watcher                      │ flush    │
  │    edits  │ (notify)                     │ to       │
  │           │                              ▼ disk     │
  │                                                     │     
  │  Control doc (circle-wide coordination)             │     
  │  ┌─────────────────────────────────────────────┐    │     
  │  │  tasks         →  Y.Map  (task_id → Task)   │    │    
  │  │  presence      →  Y.Map  (agent_id → info)  │    │     
  │  │  lock_log      →  Y.Array (append-only)     │    │     
  │  │  member_list   →  Y.Map  (peer_id → entry)  │    │     
  │  │  chat          →  Y.Array (messages)        │    │     
  │  │  mls_key_packages → Y.Map                   │    │     
  │  │  mls_welcomes     → Y.Map                   │    │     
  │  │  mls_commits      → Y.Array                 │    │     
  │  │  mls_pending      → Y.Map                   │    │    
  │  │  mls_owner_claims → Y.Map                   │    │     
  │  └─────────────────────────────────────────────┘    │     
  └─────────────────────────────────────────────────────┘     
                        │                                  
                   P2P sync                               
                   (libp2p-stream                          
                    y-sync protocol)                       
                        │                                  
                        ▼                                  
                  other daemons                            
```

Because the control doc is a CRDT, two daemons can both write to it simultaneously (e.g. both try to create a task) and the merge is deterministic and correct.

---

## A File Edit, End to End

```
  1. Alice edits src/main.rs in her editor
         │
         ▼
  2. notify (file watcher) fires a Modify event
         │
         ▼
  3. Daemon reads the file, diffs against Y.Text
     applies the diff as a Yjs transaction
         │
         ├──────────────────────────────────────────────────►  4. Y.Text observer fires
         │                                                            broadcasts raw update bytes
         │                                                            to all_updates channel
         │                                                                   │
         │                                                                   ▼
         │                                                       5. P2P sync task picks it up
         │                                                          sends to all connected peers
         │
         ▼
  6. Each peer's daemon receives the update bytes
     applies to its local Y.Text
     Y.Text observer fires → flush_to_disk
     writes src/main.rs to Bob's workspace
```

Bob's editor sees the file change. Total latency: typically under 100ms on LAN.

---

## The Control Doc, End to End

Same CRDT mechanism applies to all coordination:

```
  enox say "shipping today @bob"
      │
      ▼
  POST /circles/{id}/api/chat
      │
      ▼
  appends ChatMessage to chat Y.Array
      │
      ├──► P2P sync → bob's daemon → bob's daemon fires AgentMentioned SSE event
      │                              bob's agent wakes up
      │
      └──► local SSE stream → alice's `enox watch` terminal shows the message
```

Same pattern for tasks, locks, presence — everything flows through the CRDT and everyone converges.

---

## Member List and Trust

The member list is the source of truth for who is in the circle. Each entry is admin-signed:

```
  MemberEntry {
      peer_id:    "12D3KooW..."   ← machine identity (MLS leaf)
      owner:      "alice"         ← human owner (admin-signed, first-come-first-serve)
      agent_id:   "alice"         ← agent label (for attribution)
      role:       admin | member
      signature:  hex(admin_sign("add:{peer_id}:{role}:owner:{owner}"))
  }
```

The admin's signature covers the `owner` field. Once `owner: alice` is bound to `peer_id: 12D3...`, no other peer can claim `owner: alice` (the approve endpoint enforces uniqueness and rejects a second claim with 409).

The owner claim itself is also self-signed by the peer:
```
  OwnerClaim {
      owner: "alice"
      sig:   hex(peer_sign("owner:alice"))
  }
```
This proves the holder of that peer's private key asserts the name — preventing the admin from accidentally assigning the wrong name to a peer.

---

## Directory Layout

```
~/.enoxian/
└── circles/
    └── <circle-id>/
        ├── config.toml       circle_id, PSK, keypair, join_policy, owner
        ├── admin.key         Ed25519 hex (creator only — never shared)
        └── mls/
            ├── identity.json MLS signing keypair + credential
            └── group.json    MLS group state (serialised OpenMLS storage)

~/enoxian/                   default workspace root
└── my-project/               one directory per circle
    ├── src/
    │   └── main.rs           synced files live here
    └── notes.txt
```

The workspace (`~/enoxian/my-project/`) and the credentials (`~/.enoxian/circles/<id>/`) are intentionally separate — workspace files are easy to find and edit directly; credentials are in a dotfile dir and never appear in the workspace.

---

## Quick Reference

```
enox init --name "proj" --owner alice     create circle, you are admin
enox invite "proj"                         generate invite link
enox enter enoxian://...  --owner bob    join from invite
enoxd                                      start daemon (all known circles)

enox status                               circle overview
enox who                                  who is online
enox member list                          all members (owner / agent / role)
enox member pending                       waiting for approval
enox member approve <peer_id>             approve + MLS add + PSK rotation
enox member reject  <peer_id>             deny
enox member remove  <peer_id>             remove + MLS epoch advance + PSK rotation
enox member remove-by-owner alice         remove all of alice's machines at once

enox tasks                                task board
enox task-create "write tests"            create task
enox claim <task_id>                      claim it
enox done  <task_id>                      mark done

enox bind    src/main.rs                  acquire advisory lock
enox release src/main.rs                  release lock

enox say "hello @bob"                     post chat message
enox chat -f                              follow live chat
enox watch                                stream all circle events
```
