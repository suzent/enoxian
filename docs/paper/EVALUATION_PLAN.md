# Enoxian Evaluation & Narrative Strategy for ML-Sys / Systems Venues

This document outlines the evaluation strategies necessary to position Enoxian for top-tier systems and ML-Sys conferences (e.g., SOSP, OSDI, MLSys, NSDI).

The goal is to transition the narrative from "we built a P2P sync tool" to **"we built the fundamental state-coordination substrate for the next generation of high-concurrency LLM Multi-Agent systems."**

---

## 0. Baseline Matrix

Enoxian should not be compared against one single baseline. Each claim needs a targeted baseline.

| Claim | Enoxian baseline | Why it matters |
|---|---|---|
| Multi-agent coordination | AutoGen / CrewAI / LangGraph | Message-passing agent frameworks |
| Coding-agent workflow | SWE-agent / OpenHands / Aider | Existing software-engineering agents |
| Real-time collaboration | Yjs + centralized `y-websocket` | Centralized CRDT collaboration |
| File synchronization | Syncthing / rsync / Git | Traditional file/workspace synchronization |
| Coordination service | Redis lock / etcd / ZooKeeper | Centralized lock/state coordination |
| External memory | RAG / vector DB memory | Alternative way to reduce prompt context |
| Human-AI coding | Cursor / GitHub Copilot / ChatGPT | Turn-based human-agent interaction |

---

## Storyline A: The "Agentic vLLM" (ML-Sys Focus)

**Core Argument:** Current multi-agent frameworks (AutoGen, ChatDev) rely on JSON message-passing over centralized buses, resulting in immense token overhead, sequential blocking, and context bloat. Enoxian introduces a "CRDT + P2P Shared Workspace" that allows LLMs to interact directly via the filesystem and deterministic locks. This changes agent coordination from *message-passing* to *shared-state coordination*, reducing context overhead and accelerating wall-clock execution time.

### Evaluation 1: Wall-Clock Concurrency Speedup

* **Hypothesis:** Enoxian enables true parallel agent execution, cutting down task completion time compared to turn-based dialogue frameworks.
* **Experiment:**
  * Set up a mock software engineering task (e.g., refactoring 5 decoupled files, or frontend/backend API alignment).
  * **Baseline:** Run 5 LLM agents using a standard conversational framework.
  * **Enoxian:** Run 5 LLM agents that observe the Enoxian `events` stream and claim advisory locks to edit files concurrently.
* **Metric:** Total wall-clock time to successful compilation. Show a scaling graph: as task parallelization increases, Enoxian's completion time drops, while the baseline remains flat or fails due to context limits.

### Evaluation 2: Token Efficiency & Context Overhead Reduction

* **Hypothesis:** By externalizing coordination state into the Enoxian Control Doc and physical filesystem, agents consume fewer tokens.
* **Experiment:**
  * Trace prompt and completion tokens used during Evaluation 1.
  * Compare against message-passing, long-context prompt history, and RAG/vector-memory variants.
* **Metric:** Total token cost and coordination-token ratio. Phrase this as **"reducing context overhead by offloading coordination state to a replicated distributed substrate"**, not as a model-level context-compression algorithm.

### Evaluation 3: Breaking the "Private Information" Barrier (Collaboration Efficacy)

* **Hypothesis:** Asymmetric "ping-pong" dialogues suffer when agents hold private knowledge (as modeled in recent literature like *MT-PingEval*). A shared CRDT workspace acts as a physical substrate for mutual awareness, resolving information asymmetry more efficiently.
* **Experiment:**
  * Implement a multi-agent collaboration scenario inspired by MT-PingEval (arXiv:2602.24188), where agents have separate private goals or context but must cooperate on a shared objective.
  * **Baseline:** Agents coordinate via message passing.
  * **Enoxian:** Agents coordinate by updating the Control Doc, task board, and shared files via `enoxd`.
* **Metric:** Communication turns to success, task success rate, mistaken assumptions about peer state, and hallucinated coordination actions.

### Evaluation 4: Software Engineering Integrity & Rework Rate (ICSE/ASE Focus)

* **Hypothesis:** Enoxian's lock-based concurrency prevents semantic interleaving, leading to higher codebase stability when AI agents batch-edit files.
* **Experiment:**
  * Run CI test suites automatically after every batched proposal/commit during a multi-agent refactoring task.
* **Metric:**
  * **Codebase Compilation Integrity over Time:** Percentage of intermediate commits that pass compilation.
  * **Test Pass Rate over Time:** Percentage of intermediate states passing unit/integration tests.
  * **Rework/Reversion Rate:** How often an agent's proposal is rejected, reverted, or overwritten by a human/agent.
  * **Conflict Resolution Cost:** Time and token cost required to repair conflicting edits.

---

## Storyline B: The High-Concurrency Deterministic Substrate (Systems/OSDI Focus)

**Core Argument:** Optimistic concurrency (Last-Writer-Wins) and pessimistic centralized locks fail under the bursty, high-throughput write profiles of AI agents. Enoxian's decentralized, log-replay advisory locking combined with CRDTs provides a deadlock-free coordination primitive without a central arbiter.

### Evaluation 5: High-Contention Lock Arbitration & Tail Latency

* **Hypothesis:** The deterministic First-Writer-Wins (FWW) log replay algorithm resolves lock contention correctly without excessive performance overhead.
* **Experiment:**
  * Spawn 10-50 nodes competing for the same lock simultaneously.
  * Increase `lock_log` length from 10³ to 10⁶ entries.
* **Metric:** P50/P95/P99 latency of `compute_lock_state()`, lock acquisition latency under contention, CPU cost, memory footprint, and identical holder-map rate across nodes.

### Evaluation 6: Time-to-Consistency (TTC) under Network Jitter (Mininet/tc)

* **Hypothesis:** The libp2p network and Yjs merge engine scale under realistic WAN conditions.
* **Experiment:**
  * Spin up 5, 10, 20, and 50 `enoxd` daemons.
  * Use Linux `tc` or Mininet to inject 50-100ms latency, jitter, 1-5% packet loss, packet reordering, and bandwidth caps.
  * Inject high-frequency API requests simultaneously across nodes.
* **Metric:** Global Time-to-Consistency (TTC), stale-read window, update loss rate, reconnect time, and P99 convergence latency.

### Evaluation 7: Ablation Study

* **Hypothesis:** Enoxian's performance and correctness come from the combination of P2P, CRDT, advisory locks, and the proposal layer; removing any component weakens the system.
* **Variants:**
  * **Enoxian-full:** P2P + CRDT + locks + proposal layer.
  * **No-lock:** CRDT only; no advisory locks.
  * **No-proposal:** All agent edits go through `Y.Text` CRDT.
  * **Centralized-sync:** Keep Enoxian APIs but replace P2P with centralized `y-websocket`.
  * **Message-only:** Tasks/messages only; no shared filesystem state.
* **Metric:** Task success rate, compile pass rate, semantic conflict rate, token cost, wall-clock time, and bandwidth.

---

## Storyline C: Robustness, Offline-First, and Human-AI Dynamics

**Core Argument:** AI agents are increasingly deployed in edge environments or alongside humans. Enoxian decouples network transport from trust, enables continuous offline work, and establishes observable shared state for human-AI teaming.

### Evaluation 8: Pull-Based Anti-Entropy vs. Eager Push (Bandwidth Optimization)

* **Hypothesis:** The Proposal layer's shift from CRDT eager-push to pull-based anti-entropy drastically reduces network I/O upon reconnection.
* **Experiment:**
  * Run Node A and Node B. Sever the connection using `tc` or firewall drops.
  * Inject 1,000 heavy proposals into Node A while isolated.
  * Restore the connection.
* **Metric:** Network bandwidth peak, total bytes transferred, time-to-proposal-convergence, duplicate transfer ratio, and reconnect CPU cost.

### Evaluation 9: Misbehaving Agent Robustness

* **Hypothesis:** Enoxian remains recoverable under buggy or stale agents, even if it does not claim Byzantine consensus.
* **Faults:**
  * **Lock violator:** Writes without claiming a lock.
  * **Zombie agent:** Claims a lock and crashes.
  * **Spam agent:** Floods tasks/proposals/chat.
  * **Stale agent:** Rejoins after long offline work with outdated state.
  * **Partial writer:** Crashes mid-file-write.
* **Metric:** Corrupted file count, lock recovery time, proposal rejection rate, task queue pollution, time-to-healthy-state, and manual repair burden.

### Evaluation 10: Repository Scale Stress Test

* **Hypothesis:** Enoxian remains practical on real software workspaces, not only toy examples.
* **Experiment:**
  * Test Toy (50 files / 5k LOC), Medium (1k files / 100k LOC), and Large (10k+ files / 1M+ LOC) repositories.
* **Metric:** Initial sync time, memory footprint per daemon, file watcher overhead, proposal diff generation time, control-doc growth, reconnect time, and idle CPU usage.

### Evaluation 11: MToM Awareness & Human Cognitive Load (CSCW Focus)

* **Hypothesis:** Viewing the Enoxian Event Stream and shared Task Board reduces the cognitive load associated with guessing what an AI agent is doing.
* **Experiment:**
  * Conduct a user study with developers pairing with an AI agent to fix a bug.
  * **Baseline:** Developer uses a chat-based assistant (e.g., ChatGPT/Cursor).
  * **Enoxian:** Developer works in an editor while monitoring Enoxian events/task board.
* **Metric:**
  * **NASA-TLX:** Subjective cognitive load and frustration.
  * **Intent Accuracy / Flash-Freeze Test:** Pause the task randomly and ask the human, "What file is the AI modifying right now? What is it trying to do?"
  * **Idle Blocking Time:** Fraction of time the human is waiting for the agent rather than making progress.
  * **Intervention Cost:** Time required to stop, redirect, or repair an agent action.

---

## Storyline D: Trust Boundary and Security Evaluation

**Positioning:** Security should not be the main paper claim unless Enoxian implements content-layer encryption and has a formal threat model. However, it is **not merely an add-on**: the paper already makes a strong systems claim, **"transport is not trust"**, so membership, revocation, and trust-boundary behavior must be evaluated enough to prevent reviewer attacks.

### Evaluation 12: Membership Revocation & Tombstone Gate

* **Hypothesis:** A removed peer holding the old PSK cannot start new sync sessions once the tombstone has propagated.
* **Experiment:**
  * Create a 3-node Circle; remove Node C via MLS membership update.
  * Let Node C attempt to reconnect using the old invite/PSK.
  * Vary tombstone propagation delay and network partitions.
* **Metric:** Revocation enforcement latency, unauthorized sync attempts blocked, number of updates leaked before tombstone arrival, and false-positive rejection rate for valid peers.

### Evaluation 13: Trust-Boundary and Relay Metadata Leakage

* **Hypothesis:** Bootstrap/rendezvous/relay infrastructure assists connectivity but does not receive workspace content.
* **Experiment:**
  * Route traffic through relay/rendezvous servers and inspect what information is observable at the relay.
  * Repeat with direct P2P connections and relayed connections.
* **Metric:** Content leakage (expected: zero plaintext workspace content), observable metadata (peer IDs, timing, volume), and relay-visible connection graph.

### Evaluation 14: Local API Attack Surface

* **Hypothesis:** The localhost API is a privileged control plane; without hardening, local malware or misconfigured CORS could control the workspace.
* **Experiment:**
  * Attempt cross-origin browser requests to `localhost:36521`.
  * Attempt unauthenticated local task creation, lock acquisition, and file mutation.
  * Repeat after API hardening (loopback-only, CORS restrictions, local auth token).
* **Metric:** Unauthorized local API operations accepted/rejected, CORS bypass success, auth failure latency, and compatibility impact on CLI/agent clients.

---

## Storyline E: Developer Experience and Agent-Agnostic Integration

**Core Argument:** Enoxian is not only a protocol; it is a low-friction substrate that lets arbitrary agents participate by producing filesystem effects.

### Evaluation 15: Agent Integration Cost

* **Hypothesis:** Enoxian reduces integration burden compared with framework-specific agent SDKs.
* **Experiment:**
  * Integrate a shell script, Aider/OpenHands-like coding agent, and a custom LLM script into Enoxian.
  * Compare against building equivalent coordination with AutoGen/LangGraph adapters.
* **Metric:** Lines of integration code, integration time, number of Enoxian-specific API calls required, ability to operate without an SDK, and failure modes during integration.

---

## Existing Benchmarks to Reuse

Enoxian should not rely only on custom microbenchmarks. Existing agent and software-engineering benchmarks can provide external validity. The best strategy is to combine **standard benchmark tasks** with Enoxian-specific metrics such as token overhead, wall-clock time, lock conflicts, proposal acceptance, and shared-state convergence.

### Recommended Benchmark Stack

| Benchmark | Priority | Fit for Enoxian | What it proves | Notes |
|---|---:|---:|---|---|
| **SWE-bench Lite / Verified** | Very high | Very high | Coding-agent success, realistic repo edits, proposal workflow | Best main coding benchmark. Use a 30-50 task subset first. |
| **MultiAgentBench / MARBLE** | Very high | Very high | Multi-agent coordination quality | Treat Enoxian as a new coordination protocol compared with star/chain/tree/graph/chat. |
| **MT-PingEval-inspired** | High | High | Private-information grounding efficiency | Best support for Evaluation 3: shared-state grounding vs ping-pong dialogue. |
| **Terminal-Bench** | High | Medium-high | Real CLI task robustness | Good fit for daemon/CLI/filesystem workflow. |
| **MLE-bench** | Medium | Medium | ML engineering multi-agent workflows | Useful case study; potentially expensive. |
| **ML-Dev-Bench / ML-Bench** | Medium | Medium | Repo-scale ML development / execution | Good for workflow and artifact-sharing claims. |
| **AgentBench** | Medium | Medium | General LLM-as-agent tool use | Mostly single-agent; useful if adapted to multi-agent shared workspace. |
| **RepoBench** | Medium | Medium | Repository-level context efficiency | Useful for token/context-overhead evaluation, not core collaboration. |
| **Overcooked / MToM shared-workspace tasks** | Medium | Medium | Human-AI shared workspace and MToM | Best for HCI/CSCW version, but requires user study or simulation. |
| **GAIA** | Low-medium | Low-medium | General assistant long-horizon tool use | Useful supplement, not core Enoxian value. |
| **WebArena** | Low | Low-medium | Web automation state tracking | Less aligned with filesystem substrate. |
| **OSWorld** | Low | Low-medium | GUI computer-use agents | Heavy setup; future expansion rather than main paper. |

### 1. SWE-bench Lite / Verified

**Use as the main software-engineering benchmark.** SWE-bench gives real GitHub issue-resolution tasks and a standardized `% Resolved` metric. Enoxian can be evaluated by wrapping a multi-agent workflow around each instance:

* planner agent analyzes issue and repository;
* implementation agent edits code;
* test agent runs tests and writes failure summaries;
* review agent inspects proposal diffs;
* Enoxian coordinates tasks, locks, proposal review, and shared files.

**Baselines:** mini-SWE-agent, SWE-agent, Aider, OpenHands, AutoGen/LangGraph multi-agent variants.

**Metrics:** `% Resolved`, wall-clock time, token cost, intermediate compile/test pass rate, proposal acceptance rate, duplicate work count, conflict/rework rate.

### 2. MultiAgentBench / MARBLE

**Use as the main multi-agent coordination benchmark.** MultiAgentBench is designed to evaluate collaboration and competition among LLM agents and already compares coordination protocols such as star, chain, tree, graph, group discussion, and cognitive planning.

**Enoxian framing:** evaluate Enoxian as an additional coordination protocol: **shared-state workspace coordination**.

**Baselines:** star, chain, tree, graph, group discussion, message-only AutoGen.

**Metrics:** task score, milestone achievement rate, coordination turns, token cost, redundant actions, conflict count, time to shared plan.

### 3. MT-PingEval-inspired Private-Information Games

**Use to support Evaluation 3.** The key question is whether Enoxian reduces the cost of establishing common ground when agents start with asymmetric private information.

**Conditions:**

1. non-interactive summarize-and-act baseline;
2. multi-turn chat baseline;
3. message + RAG/vector-memory baseline;
4. Enoxian shared-state coordination;
5. Enoxian no-lock ablation.

**Metrics:** success rate, turns to success, tokens to success, information density, mistaken assumptions, time-to-common-ground, public artifact count, private-to-public artifact conversion efficiency.

### 4. Terminal-Bench

**Use for terminal-native agent workflows.** Enoxian is naturally CLI/filesystem-oriented, so Terminal-Bench-like tasks can demonstrate robust multi-agent terminal work.

**Workflow:** split a terminal task into explorer / implementer / runner / reviewer agents coordinated by Enoxian tasks and locks.

**Metrics:** task success, command failure recovery time, log-sharing token reduction, wall-clock speedup, partial-write recovery.

### 5. MLE-bench / ML-Dev-Bench / ML-Bench

**Use as optional ML engineering case studies.** These benchmarks involve ML development, dataset processing, training, experiment tracking, and report generation — naturally multi-agent workflows.

**Possible roles:** data agent, modeling agent, experiment agent, evaluation agent, report agent.

**Metrics:** final score/rank, experiment throughput, artifact reuse, checkpoint/log synchronization overhead, wall-clock speedup, token cost.

### 6. AgentBench

**Use as a general agent capability supplement.** AgentBench covers OS, database, knowledge graph, card game, lateral thinking, ALFWorld, WebShop, and Mind2Web-style tasks. It is useful for demonstrating that Enoxian can orchestrate tool-using agents, but it is mostly single-agent unless adapted.

**Metrics:** task success, tool-call efficiency, coordination overhead if adapted to multiple agents.

### 7. RepoBench

**Use for context-overhead analysis.** RepoBench evaluates repository-level retrieval and code completion. Enoxian can use it to test whether shared workspace access reduces prompt size and improves cross-file awareness.

**Metrics:** prompt length reduction, retrieval/query count, cross-file context error rate, completion success.

### 8. Human-AI / MToM Shared-Workspace Tasks

**Use for an HCI/CSCW-oriented version.** Overcooked-style shared workspace tasks can test whether observable actions and artifacts reduce reliance on verbal communication.

**Metrics:** NASA-TLX, intent accuracy, idle blocking time, intervention cost, trust calibration, perceived agency, and shared-awareness accuracy.

### Suggested Execution Phases

**Phase 1: Minimal publishable core**

1. SWE-bench Lite / Verified subset.
2. MultiAgentBench / MARBLE adapter.
3. MT-PingEval-inspired private-information tasks.
4. Custom Enoxian systems microbenchmarks.

**Phase 2: Systems credibility**

1. Terminal-Bench-like CLI tasks.
2. TTC under `tc`/Mininet.
3. P99 lock arbitration and anti-entropy bandwidth.

**Phase 3: Optional expansion**

1. MLE-bench / ML-Dev-Bench / ML-Bench.
2. RepoBench.
3. Overcooked / MToM user study.
4. GAIA / WebArena / OSWorld only if the paper shifts toward general agent infrastructure.

---

## Priority Recommendation

If time is limited, prioritize:

1. **SWE-bench Lite / Verified subset** — validates the coding-agent substrate story.
2. **MultiAgentBench / MARBLE** — validates Enoxian as a multi-agent coordination protocol.
3. **MT-PingEval-inspired tasks** — validates Evaluation 3 and the shared-state grounding claim.
4. **Ablation Study** — proves each architectural component is necessary.
5. **Misbehaving Agent Robustness** — prevents reviewers from dismissing the system as happy-path only.
6. **Membership Revocation & Tombstone Gate** — supports the paper's "transport is not trust" claim.
7. **TTC under `tc`/Mininet** — provides credible systems evidence beyond localhost tests.
