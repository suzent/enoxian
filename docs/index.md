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
| [guide/releasing.md](guide/releasing.md) | CI jobs, release gates, checksummed installers, and Homebrew automation |

## Reference

| File | Description |
|------|-------------|
| [reference/api.md](reference/api.md) | Local REST/SSE/WebSocket API exposed by Enoxian |
| [reference/daemon.md](reference/daemon.md) | Daemon/service startup, config files, routes, and environment variables |
| [reference/protocol.md](reference/protocol.md) | Yjs sync WebSocket and event-stream protocol details |
| [reference/p2p-protocols.md](reference/p2p-protocols.md) | Versioned peer wire formats, encryption, and limits |
| [reference/rendezvous-setup.md](reference/rendezvous-setup.md) | Deploying and using a bootstrap rendezvous/relay server |

## Concepts

| File | Description |
|------|-------------|
| [concepts/overview.md](concepts/overview.md) | High-level tour of enoxian |
| [concepts/concepts.md](concepts/concepts.md) | Circle, identity, CRDT, proposal, event, and coordination vocabulary |
| [concepts/architecture.md](concepts/architecture.md) | Runtime components, state surfaces, and data flow |
| [concepts/internals.md](concepts/internals.md) | Watcher, persistence, peer-session, and agent mechanics |
| [concepts/proposals.md](concepts/proposals.md) | File capture, accepted history, pending compatibility, diff, merge, and revert |
| [concepts/storage.md](concepts/storage.md) | Workspace and circle persistence, retention, and at-rest limitations |
| [concepts/security.md](concepts/security.md) | Trust model, identity, PSK, Noise, and MLS content protection |

The documentation describes current behavior. Completed design plans and old
roadmaps are retained in Git history rather than kept as a second, stale source
of truth.
