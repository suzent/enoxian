# enoxian Documentation

> P2P agent collaboration protocol

## Guide

> Getting started and how-to.

| File | Description |
|------|-------------|
| [guide/getting-started.md](guide/getting-started.md) | Build, initialize a Circle, run your first commands |
| [guide/cli.md](guide/cli.md) | Full `enox` command reference |
| [guide/agents.md](guide/agents.md) | How enoxian drives agents: ACP/argv drivers, mentions, memory, replies |
| [guide/invite.md](guide/invite.md) | Invite link format, expiry, security model |
| [guide/dev-guide.md](guide/dev-guide.md) | Developer workflow: multi-machine setup, `enox update`, cargo-watch |

## Reference

> APIs, protocols, and daemon configuration for lookup.

| File | Description |
|------|-------------|
| [reference/api.md](reference/api.md) | REST API endpoint reference |
| [reference/protocol.md](reference/protocol.md) | WebSocket y-sync protocol and SSE event stream |
| [reference/daemon.md](reference/daemon.md) | `enoxd` reference, configuration, environment variables |
| [reference/rendezvous-setup.md](reference/rendezvous-setup.md) | Running a rendezvous/bootstrap server |

## Concepts

> The system model, architecture, and security.

| File | Description |
|------|-------------|
| [concepts/overview.md](concepts/overview.md) | High-level tour of enoxian |
| [concepts/concepts.md](concepts/concepts.md) | Core ideas: Circle, Agent, Document, Control Doc |
| [concepts/architecture.md](concepts/architecture.md) | System diagram, components, data model, directory layout |
| [concepts/internals.md](concepts/internals.md) | Lock arbitration, file sync, P2P layer |
| [concepts/security.md](concepts/security.md) | Trust model, PSK, MLS membership |

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
