# Driving Agents

How enoxian runs AI coding agents — Claude Code, Codex, and any other tool —
against a circle's shared workspace, and turns their work into reviewable
proposals and chat replies.

The guiding principle: **agents do not need to understand enoxian.** enoxian
captures their filesystem effects and, where the tool supports it, drives a real
conversation. A chat `@mention` is only *intent* — whether it runs anything on a
given device is that device's own local decision.

---

## The big picture

```text
@mention in circle chat
      │  (ordinary replicated chat message — not a command)
      ▼
this device's reaction policy   ── pull ─▶  do nothing (agent must act on its own)
      │  push
      ▼
allowlist check (agents.toml)   ── no match ─▶  ignore
      │  match
      ▼
local execution layer
      ├── acp driver   ── ACP over stdio: session, streaming reply, memory
      └── argv driver  ── spawn command, capture whatever it writes
      │
      ▼
results
      ├── file changes  ─▶  a reviewable proposal (CHANGES tab / `enox proposal list`)
      └── text reply     ─▶  posted back into circle chat as the agent
```

Two things are deliberately separate:

- **Who can ask** — any circle member can mention an agent. That is just chat.
- **Who runs it** — only the target device's local daemon, under its own
  policy. A remote member can never force execution on your machine.

See [plan/agent-workspaces.md](plan/agent-workspaces.md) for the design rationale
behind this split.

---

## Configuration: `~/.enoxian/agents.toml`

This file is **device-local and never synced**. It answers two questions for
*this* device: how it reacts to mentions, and which agents it may run.

```toml
# How this device reacts to an @mention of one of its agents:
#   "pull" (default) — do nothing automatically; an agent is expected to read
#                      chat and act on its own. Safe: no mention runs anything.
#   "push"           — auto-launch the mentioned agent.
reaction = "push"

[agents.claude]
driver = "acp"
command = ["npx", "@zed-industries/claude-code-acp"]

[agents.codex]
driver = "acp"
command = ["npx", "@agentclientprotocol/codex-acp"]
```

- The **table key** (`claude`, `codex`) is the name you mention: `@claude …`.
- `command` is argv — no shell, so no quoting or injection concerns.
- `working_dir` (optional) is relative to the workspace root.

The allowlist is the security gate: a mention of an agent not listed here is
ignored. Missing file = pull, no agents = the device reacts to nothing. The
daemon reloads this file **per mention**, so edits take effect without a restart.

A read-only view of the effective config is in the frontend under the device
badge → **Device Settings** (editing stays file-only, on purpose — `push` is the
toggle that lets a mention run a local process).

See [examples/agents.toml](examples/agents.toml) for a fuller annotated example.

---

## The two drivers

Every agent is launched through one of two drivers, chosen per agent in config.

### `acp` — Agent Client Protocol (recommended)

For agents that speak [ACP](https://agentclientprotocol.com/). enoxian is the
**client**; the agent is the **agent**, over newline-delimited JSON-RPC on the
child's stdio. This is the rich path.

The handshake per run:

```text
initialize        advertise fs capabilities; read the agent's capabilities
session/new       open a session with cwd = the circle workspace
   (or)
session/load      resume a prior session id — restores conversation memory
session/prompt    send the task; the agent works until it returns a stop reason
```

During the prompt turn the agent may call back to enoxian:

| Agent → enoxian call    | enoxian's behavior                                         |
|-------------------------|------------------------------------------------------------|
| `fs/read_text_file`     | read a workspace file (confined to the workspace)          |
| `fs/write_text_file`    | write a workspace file (confined; captured as proposal)    |
| `session/request_permission` | always allow the agent to act *in the workspace* — safety is enforced later, at the proposal-acceptance layer, not by crippling the turn |
| `session/update`        | streamed output; the agent's message text is collected for the chat reply |

What the acp driver gives you that argv does not: a real completion signal
(stop reason), a **text reply** posted to chat, and **conversation memory** via
session resume.

Verified working adapters:

- **Claude Code** — `["npx", "@zed-industries/claude-code-acp"]` (needs Claude
  Code auth on the machine)
- **Codex** — `["npx", "@agentclientprotocol/codex-acp"]` (needs OpenAI/ChatGPT
  auth: `codex login`, or `CODEX_API_KEY`/`OPENAI_API_KEY` in the daemon's
  environment)

### `argv` — universal fallback

For any tool that does **not** speak ACP. enoxian substitutes `{{task}}` into
the command, spawns it in the workspace, and waits. The tool writes files
however it likes; the ambient snapshot engine notices the changes and turns them
into a proposal.

```toml
[agents.mytool]
driver = "argv"
command = ["mytool", "--prompt", "{{task}}"]
```

Trade-off: no streaming reply, no memory, no permission mediation — just "run
this and capture what it touched." But the agent needs to know nothing about
enoxian, which is the whole point of the fallback.

---

## What comes back

A run produces up to two independent results:

1. **A proposal** for any files the agent changed. It is attributed to the agent
   (`managed_process` confidence, since enoxian owned the process). Review it in
   the frontend **CHANGES** tab or with `enox proposal list` / `show` /
   `accept` / `reject` / `revert`. Whether it auto-accepts or waits for review is
   set by the acceptance policy (local-initiated runs auto-accept by default;
   remote-member-initiated runs wait for review).

2. **A chat reply** — for acp agents, the agent's streamed text is posted back
   into the circle chat under the agent's name, so `@claude …` reads like a
   conversation. (argv agents produce no chat reply.)

---

## Conversation memory

acp agents get continuity per **(circle, agent)**. After each run, enoxian
persists the ACP `sessionId` at:

```text
~/.enoxian/circles/<circle-id>/agent_sessions/<agent>.session
```

On the next mention of that agent in the same circle, enoxian passes the id to
`session/load` so the agent resumes with its prior history. All mentions of
`@claude` in a circle share one evolving conversation — "claude is a participant
in this room."

This is **best-effort**: the ACP spec does not guarantee an agent retains
session state across its own process restarts, so a stored id can fail to load.
When that happens enoxian falls back to a fresh `session/new` — you lose
continuity, never the run.

---

## World context

Beyond its own memory, an agent needs to know *where it is*. On a **fresh**
session enoxian prepends a standing brief to the prompt:

- what enoxian is and which circle it is in
- the member roster (owners, devices, their agents)
- that its file changes become reviewable proposals
- that its text reply goes to the circle chat
- the recent conversation in the room

On a **resumed** session the agent already has that history, so it gets only a
lean per-turn header (`{sender} mentioned you …`) plus the task.

---

## Mentions and targeting

Mentions address the member hierarchy at three levels:

```text
@claude                      bare agent — any device that allowlists `claude`
                             may react
@alice/laptop/claude         a specific device's agent — only that device reacts
@alice        @alice/laptop  a user / a device — notify only, launches nothing
```

The frontend chat box offers a `@` autocomplete over the *user → device →
agent* tree, so you can pick a target instead of typing the path. A device only
appears with agents under it if it **advertises** them — which it does
automatically for every agent in its `agents.toml`. If a device shows no agents,
it has none configured (or hasn't reconnected since configuring them).

> Advertising an agent means a matching mention will run it. Keep only agents
> you actually have installed and authenticated in `agents.toml`.

---

## Running an agent directly (no chat)

`enox agent run` drives the same execution path locally, without a mention —
useful for testing or scripted runs:

```bash
enox agent run claude "add a MIT license file"
```

It resumes the agent's remembered session, prints the reply, and the file
changes become a proposal exactly as a mention would. (It does not inject the
full world context, since it runs standalone without the live circle state.)

---

## Reference

- Config example: [examples/agents.toml](examples/agents.toml)
- Design rationale: [plan/agent-workspaces.md](plan/agent-workspaces.md)
- Proposal review: [cli.md](cli.md) (`enox proposal …`)
- Security model: [security.md](security.md)
