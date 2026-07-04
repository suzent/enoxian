# enoxian Documentation

enoxian is a P2P collaboration layer for humans and AI agents working inside a
shared Circle. Start with the practical guides, then use the reference pages for
exact CLI/API behavior.

## Start Here

| File | Description |
|------|-------------|
| [guide/getting-started.md](guide/getting-started.md) | Build from source, create a Circle, start the daemon, join another device |
| [guide/cli.md](guide/cli.md) | Complete `enox` command reference |
| [guide/invite.md](guide/invite.md) | Invite URI format, TTLs, relay/rendezvous addresses, security notes |
| [guide/agents.md](guide/agents.md) | Configuring local agents, mention reactions, ACP/argv drivers |
| [guide/dev-guide.md](guide/dev-guide.md) | Developer workflow: multi-machine setup, `enox update`, cargo-watch |

## Reference

| File | Description |
|------|-------------|
| [reference/api.md](reference/api.md) | Local REST/SSE/WebSocket API exposed by `enoxd` |
| [reference/daemon.md](reference/daemon.md) | `enoxd` startup, config files, routes, and environment variables |
| [reference/protocol.md](reference/protocol.md) | Yjs sync WebSocket and event-stream protocol details |
| [reference/rendezvous-setup.md](reference/rendezvous-setup.md) | Deploying and using a bootstrap rendezvous/relay server |

## Concepts

| File | Description |
|------|-------------|
| [concepts/overview.md](concepts/overview.md) | High-level tour of enoxian |
| [concepts/concepts.md](concepts/concepts.md) | Core ideas: Circle, Agent, Document, Control Doc |
| [concepts/architecture.md](concepts/architecture.md) | System diagram, components, data model, directory layout |
| [concepts/internals.md](concepts/internals.md) | Lock arbitration, file sync, P2P layer |
| [concepts/security.md](concepts/security.md) | Trust model, PSK, MLS membership |

## Planning

Planning documents are design notes, not the source of truth for current
behavior.

| File | Description |
|------|-------------|
| [plan/roadmap.md](plan/roadmap.md) | Current roadmap and next milestones |
| [plan/agent-workspaces.md](plan/agent-workspaces.md) | Local workspace proposal layer for ambient and triggered agents |
| [plan/control-persistence.md](plan/control-persistence.md) | Persisting chat/tasks/members so an all-offline restart doesn't lose them |
| [plan/identity.md](plan/identity.md) | Device identity, stable PSK, MLS membership, future content encryption |
