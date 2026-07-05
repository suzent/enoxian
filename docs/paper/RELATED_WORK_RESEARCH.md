# Related Work Research for Enoxian

This document organizes the literature and benchmark landscape around Enoxian's current paper direction: **a shared-state coordination substrate for high-concurrency LLM agent workspaces**.

## 1. Core Positioning

Enoxian sits at the intersection of five research threads:

1. **LLM multi-agent frameworks** — AutoGen, ChatDev, MetaGPT, CrewAI, LangGraph.
2. **Agent and software-engineering benchmarks** — SWE-bench, MultiAgentBench, AgentBench, Terminal-Bench, MT-PingEval.
3. **Human-AI shared workspace / Mutual Theory of Mind** — shared workspace tasks, implicit communication, human cognitive load.
4. **CRDT / local-first collaboration systems** — Yjs, Automerge, Logoot, local-first software.
5. **Distributed coordination and trust boundaries** — distributed locks, P2P networking, MLS membership, revocation, relay metadata.

The best framing is not "a P2P Google Docs for agents". It is:

> Enoxian externalizes multi-agent coordination state from prompt histories into a persistent, replicated, queryable workspace.

This gives it a credible ML-systems story: reducing token overhead and coordination latency by changing the substrate of collaboration.

---

## 2. LLM Multi-Agent Frameworks

### AutoGen

AutoGen is a natural baseline because it explicitly frames multi-agent LLM applications as **multi-agent conversation**. It allows developers to compose customizable, conversable agents using LLMs, tools, and human inputs. Its strength is flexibility; its limitation for Enoxian's argument is that coordination is still primarily conversation/message based.

**How to use in paper:**

- Baseline for message-passing multi-agent coordination.
- Contrast: AutoGen agents coordinate by conversations; Enoxian agents coordinate by durable workspace state.
- Evaluation: token overhead, turns-to-success, duplicate work, stale state.

### ChatDev / MetaGPT / LangGraph / CrewAI

These systems are useful as conceptual baselines rather than necessarily all requiring direct implementation. They represent role-specialized agent teams, graph/state machine orchestration, and centralized workflow management.

**How to use in paper:**

- Discuss them as application/framework-level coordination.
- Enoxian is lower-level: it is a substrate that these frameworks could run on top of.

---

## 3. Agent Benchmarks

### SWE-bench / SWE-bench Lite / SWE-bench Verified

SWE-bench is the strongest software-engineering evaluation target. It evaluates whether agents can resolve real GitHub issues. Verified contains a human-filtered subset of 500 instances; Lite is a lower-cost subset. The primary metric is `% Resolved`.

**Why it matters for Enoxian:**

- Real repository edits.
- Multi-file patches.
- Test-based evaluation.
- Proposal layer naturally maps to patch review.

**Recommended setup:**

1. Single-agent baseline: mini-SWE-agent / SWE-agent / Aider / OpenHands.
2. Message-passing multi-agent baseline: AutoGen / LangGraph wrapper.
3. Enoxian multi-agent: planner, implementer, tester, reviewer sharing a Circle.

**Metrics:**

- `% Resolved`
- wall-clock time
- total tokens
- coordination-token ratio
- intermediate compile/test pass rate
- proposal acceptance rate
- rework/reversion rate
- duplicate work count

### MultiAgentBench / MARBLE

MultiAgentBench is especially relevant because it evaluates LLM-based multi-agent systems across interactive scenarios and measures not only task completion but also collaboration/competition quality. It also evaluates coordination protocols such as star, chain, tree, and graph topologies.

**Why it matters for Enoxian:**

- Enoxian can be introduced as another coordination protocol: shared-state workspace coordination.
- Supports the claim that the substrate changes collaboration dynamics.

**Metrics:**

- task score
- milestone achievement
- coordination turns
- token cost
- redundant action count
- conflict count
- time to shared plan

### MT-PingEval

MT-PingEval evaluates multi-turn collaboration with private-information games. It is directly aligned with the claim that dialogue is an inefficient way to establish common ground when agents hold different private information.

**Why it matters for Enoxian:**

- It isolates the problem of private information and multi-turn collaboration.
- Enoxian can be evaluated as a way to turn private information into public artifacts rather than repeated chat.

**Recommended conditions:**

1. non-interactive summarize-and-act
2. multi-turn chat
3. chat + RAG/vector memory
4. Enoxian shared-state coordination
5. Enoxian without locks

**Metrics:**

- success rate
- tokens to success
- turns to success
- mistaken assumptions
- time to common ground
- number of durable workspace artifacts

### AgentBench

AgentBench evaluates LLM-as-agent capabilities across OS, DB, KG, digital games, ALFWorld, WebShop, and web browsing tasks. It is broader than Enoxian's core story but useful as a supplement.

**Use case for Enoxian:**

- Demonstrate that Enoxian can orchestrate tool-using agents.
- Best if adapted from single-agent to multi-agent shared workspace.

### Terminal-Bench

Terminal-Bench-like tasks are useful because Enoxian is CLI/filesystem-native. They can demonstrate realistic command-line workflows, recovery from failures, log sharing, and role specialization.

**Use case for Enoxian:**

- Split terminal work into explorer / implementer / runner / reviewer agents.
- Measure wall-clock speedup, command-failure recovery, log-sharing token reduction.

---

## 4. Human-AI Shared Workspace and MToM

The MToM literature is useful for the human-facing side of Enoxian. A key result from recent shared-workspace human-AI collaboration work is that humans often rely more on the agent's observable behavior than on explicit verbal communication, and bidirectional verbal communication can even increase burden.

**Why it matters for Enoxian:**

- Enoxian's event stream, tasks, locks, and proposal diffs are observable behavior traces.
- This supports a design argument: workspace artifacts can provide implicit communication more efficiently than chat.

**Evaluation ideas:**

- NASA-TLX cognitive load.
- Flash-freeze intent accuracy: pause task and ask human what the agent is doing.
- Idle blocking time.
- Intervention cost.
- Trust calibration.

---

## 5. CRDT, Local-First, and Collaborative Editing

### Yjs

Yjs is a CRDT framework that exposes shared types such as maps and arrays, automatically distributes changes, and merges without merge conflicts. It is network-agnostic, supports P2P/offline editing, snapshots, undo/redo, shared cursors, and rich text editor bindings.

**How to position Enoxian:**

- Enoxian builds on the CRDT/local-first lineage.
- Difference: Yjs solves data convergence; Enoxian adds agent coordination semantics: tasks, locks, proposals, membership, and event streams.

### Local-first software

Local-first software argues for local storage, offline-first behavior, multi-device sync, collaboration, privacy, and long-term data ownership.

**How to position Enoxian:**

- Enoxian is local-first applied to agent workspaces.
- The agent setting introduces new pressure: high-frequency batch edits, generated artifacts, and autonomous actors.

### Logoot / P2P collaborative editing

Logoot is useful historical related work because it explicitly targets P2P collaborative editing to avoid costly central services. This is a strong ancestor for the P2P editing angle, though Enoxian's novelty is the agent coordination layer and proposal mechanism.

---

## 6. Distributed Coordination and Trust

### Distributed locks

Distributed lock literature is useful as contrast. Systems such as Redis, ZooKeeper, etcd, and PostgreSQL advisory locks provide mutual exclusion through a centralized or consensus-backed service. Enoxian uses advisory locks over a CRDT append-log instead.

**Important limitation to state clearly:**

- Enoxian does not claim Byzantine consensus.
- Locks are advisory and best suited for cooperative Circle members.
- Misbehaving agent experiments should test recoverability, not Byzantine safety.

### libp2p

libp2p is relevant as the P2P transport substrate. It provides a modular protocol stack for building global-scale peer-to-peer applications.

**Use in paper:**

- Explain Enoxian's transport layer.
- Do not overclaim novelty at the networking primitive level; the novelty is composition with CRDT control state and agent coordination semantics.

### MLS / RFC 9420

MLS is relevant for the membership/trust-boundary story. RFC 9420 defines a protocol for end-to-end secure group messaging, motivated by the difficulty of establishing keys for group chat settings.

**Use in paper:**

- Supports the “transport is not trust” design.
- Be precise: current implementation evaluates tombstone-based sync exclusion, not full content confidentiality.

---

## 7. Best Reference Clusters for Paper

### Cluster A — Main contrast: message-passing multi-agent systems

- AutoGen
- ChatDev
- MetaGPT
- LangGraph
- CrewAI
- MultiAgentBench

**Paper argument:** These frameworks coordinate at the application/message layer. Enoxian coordinates through replicated workspace state.

### Cluster B — Evaluation and benchmarks

- SWE-bench / SWE-bench Verified / Lite
- MultiAgentBench / MARBLE
- MT-PingEval
- Terminal-Bench
- AgentBench

**Paper argument:** Use existing benchmarks but add Enoxian-specific metrics: tokens, wall-clock, conflicts, rework, proposal acceptance.

### Cluster C — Shared workspace / HCI

- Mutual Theory of Mind in Human-AI Collaboration
- Overcooked-style shared workspace tasks
- CSCW group awareness / implicit communication literature

**Paper argument:** Enoxian gives human and agent teammates observable state, reducing reliance on costly verbal communication.

### Cluster D — CRDT/local-first systems

- Yjs
- Automerge
- Local-first software
- Logoot / WOOT / CRDT editing literature

**Paper argument:** Enoxian extends local-first collaboration from human documents to agent workspaces.

### Cluster E — Distributed coordination and security

- Distributed locks / leases / fencing tokens
- libp2p
- MLS RFC 9420

**Paper argument:** Enoxian is not a consensus lock service; it is a cooperative advisory coordination layer with explicit trust boundaries.

---

## 8. Suggested Related Work Structure for PAPER.md

A stronger Related Work section could be:

1. **Agent interoperability and multi-agent frameworks**
   - MCP / A2A / AutoGen / ChatDev / MetaGPT / LangGraph.
2. **Agent benchmarks and software-engineering agents**
   - SWE-bench, AgentBench, MultiAgentBench, MT-PingEval, Terminal-Bench.
3. **Shared-state vs message-passing coordination**
   - Common ground, coordination overhead, private information games.
4. **CRDT and local-first collaboration**
   - Yjs, Automerge, local-first, Logoot.
5. **Distributed coordination and trust boundaries**
   - locks, P2P, MLS, revocation.
6. **Human-AI shared workspaces and MToM**
   - shared workspace tasks, implicit communication, cognitive load.

---

## 9. Main Takeaway

The strongest literature-backed story is:

> Existing LLM multi-agent systems coordinate by messages; existing CRDT systems synchronize documents; existing coding benchmarks measure final task success. Enoxian connects these threads by introducing a shared-state substrate where coordination state becomes durable, queryable, replicated, and reviewable. The paper should therefore evaluate not only final success, but the cost of coordination: tokens, turns, wall-clock time, conflicts, and rework.
