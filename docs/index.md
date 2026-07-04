# enoxian Documentation

> P2P agent collaboration protocol

## Documents

| File | Description |
|------|-------------|
| [getting-started.md](getting-started.md) | Build, initialize a Circle, run your first commands |
| [dev-guide.md](dev-guide.md) | Developer workflow: multi-machine setup, `enox update`, cargo-watch |
| [concepts.md](concepts.md) | Core ideas: Circle, Agent, Document, Control Doc |
| [agents.md](agents.md) | How enoxian drives agents: ACP/argv drivers, mentions, memory, replies |
| [cli.md](cli.md) | Full `enox` command reference |
| [invite.md](invite.md) | Invite link format, expiry, security model |
| [daemon.md](daemon.md) | `enoxd` reference, configuration, environment variables |
| [api.md](api.md) | REST API endpoint reference |
| [protocol.md](protocol.md) | WebSocket y-sync protocol and SSE event stream |
| [architecture.md](architecture.md) | System diagram, components, data model, directory layout |
| [internals.md](internals.md) | Lock arbitration, file sync, P2P layer |

## Planning

> Documents in `plan/` describe active plans, design notes, and archived
> milestone history. Implemented user-facing features are documented above.

| File | Description |
|------|-------------|
| [plan/roadmap.md](plan/roadmap.md) | Current roadmap and next milestones |
| [plan/agent-workspaces.md](plan/agent-workspaces.md) | Local workspace proposal layer for ambient and triggered agents |
| [plan/control-persistence.md](plan/control-persistence.md) | Persisting chat/tasks/members so an all-offline restart doesn't lose them |
| [plan/identity.md](plan/identity.md) | Device identity, stable PSK, MLS membership, future content encryption |

## Archived Plans

| File | Description |
|------|-------------|
| [plan/archived/](plan/archived/) | Archived plan index |
| [plan/archived/milestones.md](plan/archived/milestones.md) | Completed milestone archive |
| [plan/archived/workspace.md](plan/archived/workspace.md) | Completed M1 workspace folder design |
| [plan/archived/lifecycle.md](plan/archived/lifecycle.md) | Completed M4 circle lifecycle design |
| [plan/archived/admin.md](plan/archived/admin.md) | Completed M6 admin and member management design |
