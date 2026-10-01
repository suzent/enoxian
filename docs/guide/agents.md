# Using agents

How enoxian runs AI coding agents — Claude Code, Codex, and any other tool —
against a circle's shared workspace, and turns their work into reviewable
proposals and chat replies.

The guiding principle: **agents do not need to understand enoxian.** enoxian
captures their filesystem effects and, where the tool supports it, drives a real
conversation. A chat `@mention` is only *intent* — whether it runs anything on a
given device is that device's own local decision.

---

## Set up your first agent

For Claude Code, install and authenticate the official Claude CLI first
(`claude auth login`), and have Node.js 22+ with npm available. Then:

```sh
enox agent install claude
enox agent reaction push
enox say "@claude summarize the shared notes and suggest next steps"
```

`push` enables automatic runs for configured agents mentioned in Circle chat.
Any member can make those requests. To disable automatic mention execution on
this device:

```sh
enox agent reaction pull
```

Use the activity panel or `enox runs` to follow a run. Its reply appears in chat;
file edits appear in **HISTORY**. See [reviewing changes](../concepts/proposals.md)
for the difference between live edits and review status.

The following sections cover other agents and optional configuration.

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

ACP agents support streamed chat replies, conversation memory, and permission
requests. You can follow their progress in the activity panel or with `enox runs`.
Agent permissions depend on the configured agent and execution path; review them
before enabling automatic reactions. File history is not an approval barrier
before a tool runs.

#### Follow or stop a run

As long as it needs. ACP has no heartbeat, and a turn that says nothing for
minutes is usually a long tool call (a build, a test suite), which is work. So
enoxian never stops a turn for taking long. It watches what the agent reports
instead — each tool call it has open, and when it last said anything — and
shows it in the activity panel and in `enox runs`. A turn quiet for ten
minutes with no tool call open is flagged as possibly stuck; nothing more
happens unless you act.

A turn ends when the agent ends it, when its process exits, or when it is
stopped:

- **By you**: **Stop** in the activity panel, or `enox runs stop <id>`.
- **By an addressed request** for the same agent, when the running turn is an
  unaddressed one.

Stopping sends `session/cancel`, which obliges the agent to wind down and end
the turn as `cancelled`. It gets 30 seconds; after that its process is ended.
A stopped turn posts nothing, is recorded as `cancelled` (not a failure), and
keeps its conversation, so the next turn picks up where it stopped. Anything
it already changed in the workspace stays, and is reviewed like any other
change. An argv agent has no protocol to be asked through, so stopping one
ends its process.

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
- **Pi** — `pi-acp`. Same shape as the two above: the adapter is transport,
  the `pi` CLI you installed is the agent. Enoxian requires `pi` on `PATH` and
  hands the resolved executable to the adapter through `PI_ACP_PI_COMMAND`, so
  pi's own providers, models, prompts, and skills stay authoritative.
  Authenticate pi first (run `pi`, then `/login`, or export a provider API key);
  pi has no non-interactive auth check, so Enoxian verifies presence only. Needs
  system Node.js 22 or newer with npm, like the other adapters.

  ```bash
  enox agent install pi-acp
  ```

Built-in **native** plugins — a product CLI that speaks ACP itself, so there is
no adapter to install, nothing to pin, and no Node.js:

- **Suzent** — `suzent acp`. Enabling it only writes the chat handle; the CLI is
  whatever you installed, resolved on `PATH`. It is a translator over a running
  Suzent backend (`suzent serve` or `suzent start` must be up), so the turn
  executes with that install's own memory, skills, model configuration, and
  permission rules, and the circle workspace becomes the session's working
  directory. Approvals it needs arrive here as `session/request_permission`.

  ```bash
  enox agent install suzent
  ```

- **Hermes Agent** — `hermes acp`. Runs Hermes' editor-facing toolset with the
  install's own providers, memory, skills, and tools. Install Hermes with its
  ACP extra (`uv pip install -e '.[acp]'` in the install checkout) and configure
  a provider with `hermes model` first; `hermes acp --check` confirms it is
  ready.

  ```bash
  enox agent install hermes
  ```

- **OpenClaw** — `openclaw acp`. The CLI is a bridge onto an OpenClaw Gateway,
  so a Gateway must be reachable: every turn is forwarded there and runs with
  that Gateway's session state. It does not use ACP client filesystem methods —
  it writes through the Gateway — which the ambient proposal engine captures the
  same way as any other on-disk change. Exec approvals it needs during a turn
  arrive here as `session/request_permission`.

  ```bash
  enox agent install openclaw
  ```

For custom adapter or native plugin manifests, see
[agent integration details](../development/reference/agent-integration.md#plugin-manifests).

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
   frontend **HISTORY** tab or with `enox proposal list` / `show`, and undo it at
   any time with `enox proposal revert`. Unaddressed agent activity can instead
   produce pending proposals for review; those writes also reach the live folder.

2. **A chat reply** — for acp agents, the agent's streamed text is posted back
   into the circle chat under the agent's name, so `@claude …` reads like a
   conversation. (argv agents produce no chat reply.)

---

## Conversation memory

ACP agents can resume their conversation in the same Circle. Follow-up messages
can build on earlier work without repeating the whole context. This is best
effort: if the agent cannot restore its session, enoxian starts a fresh one.

---

## Shared context

Agents receive information about the Circle, its members, and recent chat.
Keep durable instructions and folder conventions in the shared `AGENTS.md`,
and use notes and hand-offs for context another device will need.

The shared folder is for knowledge and coordination. Keep code repositories
and build output in separate local checkouts, and record their locations in
the Circle. See [everyday collaboration](collaboration.md).

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

## Replying without a mention

After an agent answers you, your next message goes back to it for a few
minutes — no mention needed — and to the *same machine* that ran it. The
composer says so before you press Enter:

```text
replying to @claude · no mention needed          [ esc to exit ]
```

Esc leaves the conversation; the next message needs a mention again. Mentioning
any agent also re-arms the window, and windows are per person, so two people can
hold separate conversations with separate agents in one Circle without
disturbing each other.

Set `engagement_window_secs` in `agents.toml` to change the timeout, or `0` to
turn follow-up routing off and require a mention every time.

If the agent is already working, your message is **queued** rather than refused
— up to four per agent, delivered in order as separate turns. The strip says so:
`working, your message will be queued`.

When the window guesses wrong — two agents mid-conversation with you — use the
**reply** action on an agent's message instead. That routes to exactly that
agent, with no timer.

Replying to a *person* is different: it names no agent, so it reaches whoever
reads the room rather than nobody. Quoting a colleague to ask the room a
question is one of the most common ways people ask for help, and it should not
be the one shape that never reaches an agent.

## Agents that read the room

An agent can be given `engagement = "ambient"` so it sees every human message in
the Circle and decides for itself whether to answer:

Set in Device Settings, or by hand:

```toml
ambient = ["claude"]        # everywhere

[circles."<circle-id>"]
ambient = []                # ...except here
```

Whether an agent reads the room is a property of the *room*, so it is set per
Circle rather than per agent: the same `claude` can follow a working Circle
closely and stay out of a social one.

**This is off by default, and it is a real trade.** Under mentions, your chat
reaches a model provider only when you summon an agent. Under ambient, every
human line is sent to that agent's provider — so the roster marks ambient
agents, and everyone in the Circle can see who is listening.

The cost is managed without asking a model: messages under about two dozen
characters are skipped, an agent that spoke in the last 30 seconds is left
alone, and at most one ambient reply is offered per message. An agent with
nothing to add replies `PASS`, which posts nothing and shows as *considered and
passed* rather than silence.

An unaddressed turn is conversational, not a work order. Files it writes are
recorded as **pending** for review rather than accepted outright, and the agent
is told to say what needs doing rather than do it. It also gives way to a
request you made on purpose: if you address an agent while it is in an
unaddressed turn, that turn is stopped and your request runs next, in the same
conversation. An aside nobody asked for never holds up work someone did.

### Catching up after a gap

A device that was asleep, restarting, or disconnected comes back to a room that
moved on. It reads the most recent messages rather than all of them or none:

```toml
ambient_backlog_tail = 1    # how many of the newest still get a turn
```

Everything older is marked as read without spending a turn, so catching up
costs the same as keeping up. Messages are judged by whether this device has
already decided about them, not by how old they look — so a message from a
machine whose clock disagrees with yours, or one that arrives late, is still
read.

Several messages sent in quick succession are considered together, so a thought
typed across three lines gets one reply informed by all of them.

### When someone answers first

An unaddressed turn takes anywhere from seconds to minutes, and the room does
not wait. If another agent answers the same message while this one is writing,
the draft is **held** rather than posted or dropped: the agent is shown what was
said and decides for itself whether to drop its draft, post it unchanged, or
replace it with something that accounts for the new answer.

It is asked once per turn, so a busy room cannot keep an agent rewriting. A turn
you asked for by name is never held — you are owed its answer whatever anyone
else said in the meantime.

### Seeing what is waiting, and taking a message

Everything above pushes messages *at* agents. You can also pull. `enox inbox`
lists what this device has queued for an agent, and the room's unanswered
messages along with who is already working on each:

```text
$ enox inbox --agent reviewer
Waiting for reviewer on this device (0)

Open — nobody has answered (2)
  4f2a9c1e  suzy: how should retries back off when the peer is offline?
  9b01d7aa  alex: is the proposal from earlier safe to accept?  [claude has it]
```

Claiming a message tells every agent reading the room — on every device — to
leave it to you:

```bash
enox inbox claim 4f2a9c1e            # held for 10 minutes by default
enox inbox claim 4f2a9c1e --ttl 1800 # up to an hour
enox inbox release 4f2a9c1e          # give it back
```

A claim expires by itself, so one you forget about does not silence the message
for good; if nobody has answered by then, it goes back to the room. It only
affects agents reading the room — anyone who names an agent still gets that
agent's answer.

Agents do this too, without anyone asking: an agent that starts answering a
message is treated as holding it, so an agent on another device waiting for a
slot backs off instead of spending a turn on something already being handled.

### When an agent cannot answer

If the agent picked for a message fails — its adapter crashes, times out, or the
daemon restarts mid-turn — the message passes to another agent that reads the
room:

```toml
ambient_max_attempts = 2    # tries before giving up on a message
```

When the tries run out, a single system line says so in the room, so an
unanswered question does not look like one nobody cared about.

### Finding out why nothing happened

**Agent activity** lists what ran, and a **Not picked up** section lists what
did not, with the reason: *too short to be worth a turn*, *every agent reading
this room had just spoken*, *no agent by this name is configured on this
device*, *this device is set to pull*. Standing problems appear at the top —
including an agent named in `ambient` whose spelling does not match any
configured agent, which otherwise does nothing at all and says nothing about it.

## Agents mentioning agents

An agent's reply can mention another agent and wake it, so `@claude` can hand a
job to `@codex` without you relaying messages between two programs. An agent you
have allowed into a Circle is reachable by the other agents in it — there is no
separate switch to turn this on.

That is not a loosening of who may run what. Your device still decides, the way
it always did: an agent has to be in your `agents.toml`, and your reaction policy
has to be `push`. Delegation adds no authority beyond that; it only changes who
may spend it.

A cascade cannot run away. Every chat message carries the provenance of the
human message that started it, and three bounds apply:

- **one hand-off per reply** — an agent that names three agents wakes the
  first; the rest render as chips and do nothing;
- **no self-trigger** — an agent never wakes itself, however the mention is
  spelled;
- **a shared budget** — the whole cascade is capped at `max_relay_turns` agent
  turns, counted from the human message, and every device enforces its own cap
  against its own count. Set it in Device Settings, globally or per Circle.

Two agents going back and forth *is* allowed — that is the point — so the
budget, not the shape of the conversation, is what ends it. When the budget
runs out the remaining mentions simply go inert; nothing is posted to chat.
A new message from a person always mints a fresh budget.

If you would rather not wait for the budget, **stop chain** appears in the
activity strip while a delegated turn is running, and anyone in the Circle can
press it. It does not interrupt the turn already running — it stops every
further one, on every device. A chain that was stopped or ran out of budget
says so in the activity strip (`codex not triggered · relay budget spent`)
rather than leaving a permanent line in the transcript.

A reply that arrived through delegation is marked `via @claude` next to the
sender, so a reply nobody typed a request for is not mistaken for one that was.
Files an agent writes on another agent's behalf record the chain too — `enox
proposal list` shows who wrote them and who asked.

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
- File history model: [Reviewing changes](../concepts/proposals.md)
- Proposal review: [cli.md](cli.md) (`enox proposal …`)
- Security model: [Privacy and security](../concepts/security.md)
