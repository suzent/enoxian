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

See [Proposals and file history](../concepts/proposals.md) for how native agent
writes become accepted, revertible history.

---

## Configuration: `~/.enoxian/agents.toml`

This file is **device-local and never synced**. It answers two questions for
*this* device: how it reacts to mentions, and which agents it may run.

The recommended setup is a managed adapter plugin. Installation is an explicit,
one-time networked action; mentions only execute the pinned local binary:

```bash
enox agent plugins
enox agent install codex-acp
enox agent install claude
```

The installer writes the resolved executable into the same device-local config:

```toml
# How this device reacts to an @mention of one of its agents:
#   "pull" (default) — do nothing automatically; an agent is expected to read
#                      chat and act on its own. Safe: no mention runs anything.
#   "push"           — auto-launch the mentioned agent.
reaction = "push"

[agents.codex]
driver = "acp"
command = ["<enoxian-home>/adapters/codex-acp/1.1.14/node_modules/.bin/codex-acp"]

[agents.claude]
driver = "acp"
command = ["<enoxian-home>/adapters/claude-agent-acp/0.69.0/node_modules/.bin/claude-agent-acp"]
```

- The **table key** (`claude`, `codex`) is the name you mention: `@claude …`.
- `command` is argv — no shell, so no quoting or injection concerns.
- `working_dir` (optional) is relative to the workspace root.

The allowlist is the security gate: a mention of an agent not listed here is
ignored. Missing file = pull, no agents = the device reacts to nothing. The
daemon reloads this file **per mention**, so edits take effect without a restart.

You can edit this file three ways:

- **By hand** — it is plain TOML.
- **CLI** — `enox agent plugins`, `enox agent install`, `enox agent list`,
  `enox agent add`, `enox agent remove`,
  `enox agent reaction push|pull` (see below).
- **Frontend** — the device badge → **Device Settings** panel lets you add and
  remove agents and toggle the reaction (switching to `push` asks for
  confirmation, since it lets a mention run a local process).

> Editing via CLI or frontend rewrites the file and does not preserve comments;
> the values are kept exactly.

See [examples/agents.toml](../examples/agents.toml) for a fuller annotated example.

### Managing agents from the CLI

```bash
# Show the reaction policy and configured agents.
enox agent list

# List built-in and local plugin manifests, then install a pinned adapter.
enox agent plugins
enox agent install codex-acp
enox agent install claude

# Add (or replace) an agent. Everything after `--` is the launch command.
# This remains available for custom, already-installed executables.
enox agent add my-acp --driver acp -- /path/to/my-acp-adapter

# A non-ACP tool via the argv driver.
enox agent add mytool --driver argv -- mytool --prompt "{{task}}"

# Remove an agent.
enox agent remove codex

# Set how this device reacts to mentions.
enox agent reaction push    # auto-run mentioned agents
enox agent reaction pull    # do nothing on mention (default)
```

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

Built-in managed adapter plugins:

- **Claude Code via ACP bridge** — `claude-agent-acp`. The adapter is only the
  transport: Enoxian requires the official `claude` CLI, verifies
  `claude auth status`, and passes the resolved executable through
  `CLAUDE_CODE_EXECUTABLE`. This preserves the user's Claude subscription,
  `CLAUDE_CONFIG_DIR`, native settings, MCP configuration, and project skills.
  Install the CLI and run `claude auth login` before `enox agent install claude`.
  The bridge also requires system Node.js 22 or newer with npm. Enoxian checks
  these prerequisites before installation but does not install or manage them.
- **Codex** — `codex-acp` (needs OpenAI/ChatGPT
  auth: `codex login`, or `CODEX_API_KEY`/`OPENAI_API_KEY` in the daemon's
  environment)

Plugin manifests are TOML files in `~/.enoxian/plugins/`. A manifest declares
an id, exact package version, executable name, agent name, and driver. Packages
are installed under `~/.enoxian/adapters/<id>/<version>/`. Version ranges are
rejected, and plugin installation never happens while processing an `@mention`.
Legacy `npx`/`npm` agent commands are shown as **runtime download** in Device
Settings so they can be migrated with one click.

The legacy `claude-code-acp` plugin id and command remain accepted as migration
aliases, but new installations use `@agentclientprotocol/claude-agent-acp`.
This path drives Claude through the Agent SDK rather than recreating the
interactive Claude terminal UI; it nevertheless executes against the installed
Claude Code runtime and its authentication/configuration.

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

1. **Accepted proposal history** for any files the agent changed. Agent writes
   already land in the live workspace, so enoxian records the resulting diff as
   accepted rather than presenting a misleading approval gate. Inspect it in the
   frontend **CHANGES** tab or with `enox proposal list` / `show`, and undo it at
   any time with `enox proposal revert`. The pending status remains supported for
   historical records and future isolated/staged workflows.

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

- Config example: [examples/agents.toml](../examples/agents.toml)
- File history model: [proposals.md](../concepts/proposals.md)
- Proposal review: [cli.md](cli.md) (`enox proposal …`)
- Security model: [security.md](../concepts/security.md)
