# Enoxian: A Shared-State Coordination Substrate for High-Concurrency LLM Agent Workspaces

## Abstract

Contemporary multi-agent frameworks coordinate large language model (LLM) agents
predominantly through centralized message passing, request/response APIs, and
conversation histories serialized back into each model's prompt. This is
convenient for tool invocation, but it becomes inefficient for *teams* of agents
and humans working on a shared body of files: coordination state is repeatedly
re-explained in tokens, interactions become turn-based and opaque, and concurrent
batch edits have no native conflict-control substrate. We present **Enoxian**, a
peer-to-peer (P2P) shared-state coordination substrate that pushes collaboration
*below* the API layer into a replicated workspace shared symmetrically by AI
agents and humans. Enoxian combines a libp2p transport gated by a per-circle
pre-shared key, a conflict-free replicated data type (CRDT) layer for live text
and coordination state, deterministic advisory locks, an IETF MLS (RFC 9420)
membership layer with a tombstone-based eviction gate, and an agent-agnostic
*proposal* layer that captures arbitrary filesystem mutations as reviewable,
content-addressed snapshots. The central ML-systems thesis is that Enoxian
reduces coordination overhead by externalizing agent state from conversational
context into persistent, queryable workspace objects — tasks, locks, files,
proposals, and event streams. We describe the implemented system — discovery,
transport, document and control-state sync, advisory locking, membership
cryptography, and the proposal pipeline — and distinguish it from the planned
content-encryption and event-log layers. We argue that the design commitments
*transport is not trust* and *two sync surfaces* (CRDTs for interactive editing,
snapshot/diff/proposal for batch agent edits) provide the substrate needed for
high-concurrency, peer-equal human–AI and AI–AI collaboration.

---

## 1. Introduction

As LLM-based agents move from closed sandboxes into open, multi-party settings,
the dominant integration substrate has been the application-layer API. Protocols
such as the Model Context Protocol (MCP) and various agent-to-agent (A2A) schemes
standardize how an agent calls a tool or fetches data, but they inherit the
client–server topology of the services they wrap. For a single agent invoking a
remote capability this is adequate. For a *team* — several AI agents and one or
more humans jointly editing a shared corpus of files — it is not.

We identify four recurring pain points:

1. **Centralized trust and availability.** A coordinating server is a single
   point of failure and a single point of disclosure. Collaboration stops when it
   is unreachable, and every participant must trust it with the full workspace.

2. **The turn-based collaboration bottleneck.** Conventional agent interfaces are
   asymmetric: the human prompts, the agent acts opaquely, the human inspects the
   result. The human cannot *co-inhabit* the agent's working state in real time.
   Effective human–AI teaming instead depends on a shared, continuously observable
   workspace through which each party can infer the other's state and intent — a
   *mutual theory of mind*.

3. **No native concurrency control.** When multiple agents write at high
   frequency, optimistic last-writer-wins corrupts state and pessimistic locking
   deadlocks. Coordination must be intrinsic to the shared state, not bolted on by
   a central arbiter.

4. **Coordination-state serialization overhead.** Message-passing agent systems
   repeatedly serialize task state, file state, plans, and peer activity into
   natural-language prompts. This consumes tokens, creates stale context, and
   makes common ground fragile under long-horizon collaboration.

**Contributions.** This paper makes the following contributions, all reflecting a
working implementation in Rust:

- A *shared-state coordination substrate* in which the unit of collaboration is a
  **Circle** — a replicated workspace that AI agents and humans join as peers,
  rather than a sequence of API calls or chat messages routed through a hub.
- A daemon (`enoxd`) and thin CLI (`enox`) realizing P2P discovery (mDNS +
  Kademlia), a PSK-gated libp2p transport with relay/rendezvous NAT traversal,
  CRDT-based file and coordination sync, advisory file locks with deterministic
  arbitration, presence, tasks, and chat.
- A coordination model that reduces LLM context overhead by externalizing common
  ground into persistent, queryable objects: tasks, locks, presence, files,
  proposals, and event streams.
- A separation of *transport from trust*: a stable per-circle PSK gates the
  network while membership and eviction are enforced cryptographically by a signed
  member list and an MLS-backed tombstone sync gate.
- An *agent-agnostic proposal layer* that captures arbitrary local filesystem
  mutations (from any editor, script, or AI agent that knows nothing about
  Enoxian) as content-addressed snapshots, diffs them, and replicates them for
  review via a dedicated pull-based anti-entropy protocol.

We are explicit throughout about what is implemented versus planned;
content-layer end-to-end encryption (Section 7) is designed but not yet shipped.

---

## 2. Related Work

### 2.1 Agent Interoperability Protocols

Early multi-agent communication relied on speech-act standards such as FIPA-ACL.
The recent LLM-agent wave has produced new stacks — MCP for tool/context access
and assorted A2A protocols for inter-agent messaging — surveyed in the agent
interoperability literature (e.g. *A Survey of Agent Interoperability Protocols*).
These standardize *message and tool formats* but remain client–server in
topology, providing no inherent decentralization, offline operation, or shared
mutable state. Enoxian is complementary and lower in the stack: rather than
defining how an agent calls a service, it defines a P2P substrate over libp2p in
which agents and humans hold a *replicated workspace* in common.

### 2.2 CRDTs and Real-Time State Synchronization

Conflict-free replicated data types provide deterministic eventual consistency
without a central arbiter, and underpin production collaborative editors via
libraries such as Yjs and Automerge. Enoxian uses the Rust Yjs implementation
(`yrs`) for two purposes: per-file `Y.Text` for live interactive editing, and a
single in-memory *control document* whose `Y.Map`/`Y.Array` collections hold
tasks, presence, an append-only lock log, chat, the member list, and MLS
coordination state. The novel pressure in our setting is *machine* write
frequency: an agent can emit bursts of edits that would overwhelm operational
transformation or a lock server. We address this with a CRDT control plane plus
deterministic, log-replay advisory locking (Section 4.3), and — crucially — by
*not* forcing all agent file mutation through the CRDT (Section 5).

### 2.3 LLM Multi-Agent Frameworks and Coordination Overhead

Frameworks such as AutoGen, ChatDev, MetaGPT, CrewAI, and LangGraph demonstrate
that decomposing tasks across multiple LLM agents can improve modularity and
specialization. Their default coordination substrate, however, is usually
message-passing: agents exchange natural-language messages, tool-call records,
or structured JSON through a central orchestrator. This makes coordination state
expensive because plans, file context, progress, and peer intent must be repeatedly
serialized into prompts. Enoxian targets this systems bottleneck: it keeps common
ground in replicated workspace objects rather than forcing each model to recover
common ground from conversation history.

### 2.4 Coding-Agent and Multi-Agent Benchmarks

Benchmarks such as SWE-bench, SWE-bench Verified/Lite, AgentBench,
Terminal-Bench, MultiAgentBench, RepoBench, and MT-PingEval-style private
information games provide complementary lenses for evaluating agent systems.
SWE-bench measures whether agents resolve real repository issues; MultiAgentBench
focuses on coordination quality across multi-agent protocols; MT-PingEval-style
tasks expose weaknesses in multi-turn collaboration under asymmetric private
information. Enoxian is not a new model and should not be evaluated only by raw
model accuracy. Its central empirical question is whether a shared-state substrate
improves agent *workflow efficiency*: resolved tasks per unit wall-clock time,
tokens spent on coordination, intermediate codebase integrity, and conflict or
rework rates.

### 2.5 Symmetric Human–AI Collaboration

Research on *mutual theory of mind* in human–AI collaboration argues that teams
perform best when each party can dynamically perceive the other's state and
intent in a shared context. Most deployed agent interfaces are turn-based and
asymmetric. Enoxian's Circle treats the human CLI (`enox`) and each AI agent as
peer-equal entities operating on the same replicated state, with live presence,
chat, and an SSE event stream exposing every mutation — an engineering substrate
for the shared context such theories require.

---

## 3. System Overview

The unit of collaboration is the **Circle**: a UUID-identified workspace with a
human-readable name, a 256-bit pre-shared key, a watched local directory, and an
in-memory control document. Every participant — whether a human at a terminal, a
script, or an AI agent — runs one `enoxd` daemon per workspace. The daemon is
simultaneously the P2P node, the file watcher, and a localhost HTTP/WebSocket
server. The short-lived `enox` CLI and any AI agent talk to that daemon over
loopback HTTP; the daemons talk to each other over libp2p.

```
  Alice's laptop          Bob's desktop           Alice's agent host
  ┌─────────────┐         ┌─────────────┐         ┌─────────────┐
  │  enoxd      │◄──────► │  enoxd      │◄──────► │  enoxd      │
  │  ~/project/ │  P2P    │  ~/project/ │  P2P    │  ~/project/ │
  └─────────────┘  mesh   └─────────────┘  mesh   └─────────────┘
        ▲ HTTP                  ▲ HTTP                  ▲ HTTP
   human (alice)           human (bob)            AI agent
```

A human typing `enox status` issues `GET /api/status` to the local daemon; an AI
agent makes the identical call. This symmetry — both reduced to HTTP against a
local daemon that is itself a peer in the mesh — is the engineering expression of
the symmetric-collaboration goal.

### 3.1 Identity in Three Layers

Enoxian distinguishes three identifiers because they answer different questions:

- **owner** (e.g. `alice`) — the human; groups all of one person's machines. It
  is admin-signed into membership and unique per circle.
- **peer_id** (a libp2p key) — the *daemon/device*. One per `enoxd` instance,
  derived deterministically from a stable device key (Section 4.4), persistent
  across restarts. This is what the network and MLS membership track.
- **agent_id** (e.g. `alice`, `claude-code`) — an attribution label. Several
  agents on one machine share one `peer_id` but use distinct `agent_id`s for "who
  wrote this, who holds this lock."

Chat and presence key on `agent_id`; MLS membership keys on `peer_id`; the signed
member list links the two through `owner`.

### 3.2 Protocol Stack

```
┌─────────────────────────────────┐
│        Application Layer        │  AI agents, scripts, human CLI, browser UI
├─────────────────────────────────┤
│      Coordination Layer         │  tasks / locks / presence / chat / members
│                                 │  (the control CRDT doc) + proposal layer
├─────────────────────────────────┤
│      Document Sync Layer        │  Yjs CRDT — real-time per-file text sync
├─────────────────────────────────┤
│      Membership Layer           │  signed member list + MLS + tombstone gate
├─────────────────────────────────┤
│      Transport Layer            │  TCP + PSK (pnet) + Noise + Yamux; QUIC
├─────────────────────────────────┤
│      Discovery Layer            │  mDNS (LAN) + Kademlia / rendezvous (WAN)
└─────────────────────────────────┘
```

---

## 4. Architecture and Mechanisms

### 4.1 Discovery and Transport

Peers find each other by a layered strategy: mDNS for instant same-LAN discovery,
then a rendezvous server for WAN introduction, then a circuit relay as a
NAT-traversal fallback. Each circle swarm combines three transport legs:

| Transport | PSK | Purpose |
|-----------|-----|---------|
| TCP + `pnet` PSK (XSalsa20) + Noise + Yamux | required | Direct circle-peer links on LAN/WAN |
| Circuit relay (Noise + Yamux) | none | Inbound relay for NAT-blocked peers |
| QUIC | none | Connections to bootstrap/rendezvous servers |

The PSK is applied at the transport level, *before* Noise: a peer with the wrong
PSK is dropped before any protocol negotiation. A public bootstrap server
(`enoxd --bootstrap`) runs QUIC only, holds no PSK, joins no circle, and therefore
sits outside the trust boundary — it provides rendezvous and relay, nothing more.

### 4.2 Document Sync Layer

Every file in the workspace maps to a Yjs `Doc` keyed by its relative path,
holding one `Y.Text`. Sync is bidirectional:

- **Disk → CRDT.** A `notify` watcher detects edits and applies a full-text
  replace as a `TransactionMut`. (Full replace is correct under CRDT semantics:
  concurrent full replaces merge deterministically.)
- **CRDT → disk.** An incoming update (from a local WebSocket browser client or a
  remote peer) is applied to the `Doc` and flushed to disk.

Two race hazards are handled explicitly. A *self-write suppression* handshake — a
per-path `AtomicBool` shared between the watcher and the disk-flush path — keeps
`flush_to_disk` from re-triggering the watcher in an infinite loop. *Echo
prevention* tags peer-originated updates with origin `"p2p"` so the update
observer forwards them to other local WebSocket clients but not back onto the P2P
broadcast that delivered them.

Cross-daemon file sync runs over a dedicated libp2p stream protocol,
`/enoxian/sync/1.0.0`, using the y-sync protocol with a deadlock-free three-phase
handshake (both sides exchange `SyncStep1`/`SyncStep2` for every doc, in a fixed
order so neither blocks on the other), followed by continuous update exchange. A
dedicated reader task ensures frame reads are never cancelled mid-frame; on a
broadcast lag the peer resends full CRDT state, which is idempotent because CRDT
merges are safe to re-apply.

### 4.3 Coordination Layer and Advisory Locks

Circle-wide coordination lives in the in-memory **control document**, a Yjs
`Doc` whose collections are replicated by the same P2P mechanism as files:

| Key | Type | Holds |
|-----|------|-------|
| `tasks` | `Y.Map` | Task records |
| `presence` | `Y.Map` | Agent heartbeats |
| `lock_log` | `Y.Array` | Append-only lock events |
| `chat` | `Y.Array` | Chat messages |
| `member_list` | `Y.Map` | Admin-signed member records |
| `mls_removed` | `Y.Map` | Removed-peer tombstones |

**Advisory file locks** are intentionally weakly consistent — there is no Raft/
Paxos lock service. Each acquire/release appends an immutable `LockEntry` to the
`lock_log` array. Every daemon computes the current holder map by replaying the
log from index 0 with a *first-writer-wins* rule. Because Yjs array insertion
order is stable under merge, all peers replay the same order and reach the same
holder map without a coordinator. A successful acquire additionally sets the file
read-only on disk (`chmod 444` / `FILE_ATTRIBUTE_READONLY`) so non-Enoxian tools
do not clobber a held file; the permission is restored on release. This
log-replay design is what lets high-frequency agent writes avoid the deadlocks of
pessimistic locking while remaining recoverable.

The daemon emits **Circle Events** (task created/claimed/done, lock
acquired/released, file updated, chat) over a server-sent-events stream
(`GET /api/events`), giving every participant — human or agent — a live view of
the workspace's state changes.

### 4.4 Membership Layer: Transport Is Not Trust

A defining principle is that the network gate and the membership/eviction
mechanism are *separate*. The PSK is a stable, coarse network gate distributed in
invite links; it does **not** rotate on membership change and is **not** the
revocation mechanism.

Identity is anchored in a single stable device key per install. Per-circle
connection keypairs are derived deterministically via
`HKDF(device_key, "enoxian-device-v1", "circle/<circle-id>")`, yielding a stable
peer ID per `(device, circle)` pair; Noise proves ownership during connection
setup, so a peer cannot impersonate another's peer ID.

Membership uses IETF MLS (RFC 9420) via `openmls`. Each membership change
advances an MLS epoch; the admin (holder of an Ed25519 `admin.key` generated at
`enox init`) signs each member entry. Eviction works by tombstone:

1. The admin runs the MLS `remove_member`, advancing the epoch.
2. A tombstone is written to the `mls_removed` CRDT map and the Remove commit is
   broadcast so remaining members keep MLS state in sync.
3. The P2P sync handler checks `mls_removed` *before* exchanging any CRDT data and
   rejects tombstoned peers.

This blocks a removed peer from new sync sessions even though it still holds the
stable PSK. The honest limitation, stated plainly: it does not yet provide
cryptographic secrecy against a removed peer that races a member who has not yet
received the tombstone, nor does it claw back data already on the removed peer's
disk. The MLS epoch key is currently tracked but *reserved* for the planned
content-encryption layer (Section 7); it is not derived into the transport PSK.

### 4.5 Local Daemon API

`enoxd` exposes a localhost HTTP/WebSocket API (default port 36521) that the CLI
and browser UI use. It is a privileged *control plane*, not the WAN relay path:
`/ws/yjs` syncs local browser clients with the local daemon, while cross-machine
file sync uses the libp2p stream protocol. Hardening this surface (loopback by
default, restricted CORS, local API authentication) is a tracked, not-yet-complete
milestone.

---

## 5. The Proposal Layer: Capturing Arbitrary Agent Edits

CRDT text sync is excellent for live, character-level human editing but a poor
default for what AI agents actually do: large rewrites, generated artifacts,
formatter passes, and binary files. Forcing those through a per-character CRDT
pretends every mutation is a collaborative text edit. Enoxian therefore keeps
**two distinct sync surfaces**:

| Surface | Use case | Mechanism |
|---------|----------|-----------|
| Interactive | Browser editor, small live edits | Yjs / awareness |
| Workspace proposals | AI agents, scripts, batch/binary edits | Snapshot journal + diff + proposal |

The governing principle is *agent-agnosticism*:

> Agents do not need to understand Enoxian. Enoxian only needs to capture their
> filesystem effects.

An agent (or any editor or script) mutates ordinary files. A snapshot journal
captures "before" blobs for touched paths into a content-addressed blob store;
when an idle window closes or a session finishes, Enoxian snapshots the result,
generates S0→S1 diffs, performs a three-way merge against the current canonical
state, and creates a reviewable **proposal**. Proposals can be accepted, rejected,
reverted, synced, or flagged conflicted, via CLI (`enox proposal …`), a REST API,
and a frontend review view. An acceptance policy distinguishes local triggers
(auto-accept with history) from remote-member triggers (pending review).

### 5.1 Pull-Based Proposal Replication

Replicating proposals through the control-doc CRDT map proved to be a design
mistake: the map is in memory, never pruned, and *fully* re-replicated on every
reconnect, so the entire review history — bundles, snapshots, base64 blobs — was
re-sent on each connect. This coupled a durable, ever-growing artifact to a
transport meant for small live coordination state.

The fix replaces eager push with on-demand **anti-entropy** over a sibling stream
protocol, `/enoxian/proposals/1.0.0`, behind the same `mls_removed` tombstone
gate. On each connection both sides exchange a `HAVE` set of `(id, fingerprint)`
pairs (the fingerprint hashes the status-bearing fields so status changes
propagate without resending content), each requests only the ids it lacks or
whose fingerprint differs, and the other streams just those bundles. Status
divergence (e.g. concurrent accept vs. reject while offline) is resolved by an
explicit, auditable rule — the record with the greater `(status_rank, updated_at)`
wins, where `reverted > rejected > accepted > pending` — rather than by implicit
CRDT last-writer-wins. The disk store is the durable source of truth; the protocol
transfers only what a peer is missing, and convergence does not depend on any peer
staying online.

---

## 6. Implementation Status

Enoxian is implemented in Rust on `tokio`, with `libp2p` 0.56 for transport and
protocols, `yrs` 0.26 for CRDTs, `axum` 0.8 for the local HTTP/WS server,
`openmls` for membership, and `notify` for the file watcher.

| Capability | Status |
|------------|--------|
| Circle creation, invites, `enter`/`enoxd` | Implemented |
| Device-derived stable peer identity | Implemented |
| PSK-gated libp2p transport; relay/rendezvous NAT traversal | Implemented |
| Yjs file sync (`/enoxian/sync/1.0.0`) | Implemented |
| Tasks, presence, advisory locks, chat, SSE events | Implemented |
| Signed member list, MLS membership, `mls_removed` tombstone gate | Implemented |
| Proposal layer: blob store, journal, diff, merge, review API/CLI/UI | Implemented |
| Pull-based proposal replication (`/enoxian/proposals/1.0.0`) | Implemented |
| Managed-process launch (`enox agent run`), claimed sessions | In progress |
| Local API hardening (loopback default, auth, CORS) | Planned |
| Event-log + content-blob sync; diff/merge adapters | Planned |
| Layer-4 MLS-derived content encryption | Planned |

---

## 7. Planned Work: Content-Layer Encryption

The most significant gap, and the next major milestone, is end-to-end content
encryption. Today, current circle members receive *plaintext* CRDT updates after
their peer connection is decrypted; transport encryption (Noise) protects links
but not content at rest within the membership. The planned Layer 4 derives content
keys from MLS epoch state to encrypt CRDT updates, event-log entries, proposal
metadata, and blob chunks. This would provide forward/future secrecy after member
removal — closing the "removed peer races the tombstone" gap of Section 4.4 —
while keeping transport connectivity decoupled from membership, consistent with
*transport is not trust*. Residual metadata leakage to relays (peer IDs, timing,
volume) is acknowledged and to be documented rather than eliminated.

---

## 8. Evaluation Plan

We evaluate Enoxian as a systems substrate for agent collaboration, not as a new
foundation model. The central question is whether shared-state coordination
improves workflow efficiency and robustness relative to message-passing agents and
centralized coordination services. A detailed experiment matrix is maintained in
`EVALUATION_PLAN.md`; the main paper should focus on four evaluation groups.

### 8.1 Agentic Workflow Efficiency

We will use software-engineering tasks such as SWE-bench Lite / Verified subsets
to compare three settings: (i) a single coding agent, (ii) a message-passing
multi-agent baseline using AutoGen/LangGraph-style orchestration, and (iii) an
Enoxian-coordinated multi-agent workspace. Metrics include task resolution rate,
wall-clock time, total tokens, coordination-token ratio, intermediate compile/test
pass rate, proposal acceptance rate, duplicate work, and rework rate.

### 8.2 Shared-State Grounding under Private Information

To test the claim that Enoxian reduces conversational grounding overhead, we will
run MultiAgentBench and MT-PingEval-inspired tasks in which agents hold asymmetric
private information but must converge on a joint plan. Baselines include
non-interactive summarize-and-act, multi-turn chat, and message-plus-RAG memory.
Metrics include success rate, turns to success, tokens to success, mistaken
assumptions about peer state, time to common ground, and the number of durable
workspace artifacts created.

### 8.3 Systems Performance and Ablations

We will stress the coordination substrate using high-contention lock acquisition,
large `lock_log` replay, and time-to-consistency experiments under realistic
network conditions injected by `tc` or Mininet. Metrics include P50/P95/P99 lock
arbitration latency, global convergence time, stale-read windows, bandwidth, CPU,
memory, and reconnect cost. Ablations remove advisory locks, the proposal layer,
P2P transport, or shared filesystem state to determine which mechanisms are
necessary.

### 8.4 Robustness and Trust Boundary

We will evaluate the system under buggy agents and partial failures: agents that
write without locks, crash while holding locks, flood tasks/proposals, reconnect
with stale state, or crash mid-write. We will also test the implemented trust
boundary: removed peers holding an old PSK should be unable to start new sync
sessions once tombstones propagate, and relay/rendezvous infrastructure should not
observe workspace plaintext. We do not claim end-to-end content confidentiality in
the current implementation; content-layer encryption is planned work.

---

## 9. Conclusion

Enoxian reframes multi-agent collaboration as a shared-state systems problem.
Rather than treating agent coordination as a sequence of API calls or natural
language messages, it provides a replicated workspace in which tasks, locks,
presence, files, proposals, and events become durable coordination objects. Its
core commitments — *transport is not trust* and *two sync surfaces* — address the
centralization, opacity, concurrency, and context-overhead weaknesses of current
client–server agent frameworks. The implemented Rust system already provides
P2P discovery and transport, CRDT file and control-state sync, deterministic
advisory locking, MLS-backed tombstone eviction, and an agent-agnostic proposal
pipeline with pull-based replication. Content-layer encryption remains the main
planned security milestone. We believe the key design stance — Enoxian need only
capture an agent's filesystem effects — makes it a plausible substrate for the
next generation of high-concurrency LLM agent workspaces.
