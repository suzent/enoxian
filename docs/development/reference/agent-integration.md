# Agent integration details

For setup, supported agents, and everyday use, see [Using agents](../../guide/agents.md).
This page covers adapters and the execution protocol.

## ACP execution

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
| `session/request_permission` | delegated to the execution host permission handler; proposal history does not isolate writes or provide a pre-execution approval gate |
| `session/update`        | streamed output; the agent's message text is collected for the chat reply |

What the acp driver gives you that argv does not: a real completion signal
(stop reason), a **text reply** posted to chat, and **conversation memory** via
session resume.


## Authentication and transport limits

The token-protected local API exposes `GET /api/agent-auth`,
`POST /api/agent-auth/{id}` (`action: probe|login`, optional `method_id`), and
`DELETE /api/agent-auth/{id}` to cancel. Only configured ACP commands may run;
HTTP callers cannot supply commands, arguments, or environment variables.
Jobs are cancellable, limited to four concurrent operations and five minutes.
They deny permission requests and file writes and do not submit prompts.

Initialization requires protocol major version 1. The client retains advertised
`authMethods`; agent methods use `authenticate`, while terminal methods append
advertised arguments/environment and run interactively without calling
`authenticate`. Terminal capability is advertised only when available. Private
terminal handoffs are deleted after consumption, with heartbeat supervision for
cancellation and daemon exit. Provider output and credentials are not returned
through the API. Login verification uses a fresh process because actual task
runs have independent process and actor identities.

Incoming frames and client file reads are limited to 1 MiB, queued frame bytes
to 16 MiB, and accumulated updates per turn to 16 MiB. Message queues are bounded
and overflow fails explicitly. This preserves prompt cancellation and avoids
unbounded memory growth without introducing an inactivity timeout. Queued final
updates are drained before completing a prompt response. Permission options
must match the policy decision; otherwise the response is cancelled.

## Plugin manifests

Plugin manifests are TOML files in `~/.enoxian/plugins/`. A manifest declares
an id, exact package version, executable name, agent name, and driver. Packages
are installed under `~/.enoxian/adapters/<id>/<version>/`. Version ranges are
rejected, and plugin installation never happens while processing an `@mention`.

A manifest may set `kind = "native"` for a CLI that speaks ACP itself. Enoxian
then installs nothing: `binary` is resolved on `PATH`, `args` carries the
subcommand that speaks the protocol, `install_url` says where to get the CLI,
and `package`/`version` must be omitted — there is nothing to fetch and nothing
to pin. A native handle stays by-name in `agents.toml`, so reinstalling the CLI
somewhere else cannot strand it.

```toml
id = "mytool"
agent = "mytool"
kind = "native"
binary = "mytool"
args = ["acp"]
install_url = "https://example.com/install"
about = "My tool, speaking ACP directly."
```

## Session persistence

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
If an advertised `session/load` fails, the run fails and the saved ID is
preserved, including on authentication errors. Sign in and retry. If the agent
has permanently discarded the session, remove that local `.session` file while
the agent is idle to explicitly start over. Agents without load support still
open a new session.

---


## Run attribution

Before launch, Enoxian registers the configured agent label with the local
Circle daemon. Native file writes are correlated with the persisted managed
session, while coordination CLI calls inherit a short-lived actor token through
the process environment. The token is never added to the agent prompt.


## World context

Beyond its own memory, an agent needs to know *where it is*. On a **fresh**
session enoxian prepends a standing brief to the prompt:

- what enoxian is and which circle it is in
- the member roster (owners, devices, their agents)
- that its file changes become reviewable proposals
- that its text reply goes to the circle chat
- what the shared folder is for (below)
- the recent conversation in the room

On a **resumed** session the agent already has that history, so it gets only a
lean per-turn header (`{sender} mentioned you …`) plus the task.

### What the shared folder is for

The brief tells every agent that the circle's folder is a knowledge base and
index, not a place to build software. It holds notes and working text,
decisions, hand-offs, what each device is and can do, and where things live.
Repositories, build output, dependencies and large binaries stay out: every
file syncs to every member and becomes a proposal to review. Code lives in a
checkout on a device's own disk, and the folder records where (device, path,
branch).

A circle's own layout goes in an `AGENTS.md` at the folder's root, which the
brief tells agents to read first. `enox init` writes a minimal one when the
circle is created, if the folder has none: a line on what the folder is for and
a suggested layout (`devices/<device>.md`, `notes/`, `handoffs/`). From then on
it belongs to the members. They edit it like any other file, and enoxian never
rewrites it. Tools that read `AGENTS.md` from their working directory by
themselves pick it up even when they are not run by enoxian.

---

