# Developer documentation

This section is for contributors, integration authors, and maintainers.
For installing and using enoxian, start with the [user documentation](../index.md).

## Build and contribute

- [Contributing](../../CONTRIBUTING.md) — checks and pull requests
- [Developer guide](contributing/dev-guide.md) — source builds and local development
- [Releasing](contributing/releasing.md) — CI, packaging, and release procedures
- [Agent collaboration contract](../../AGENTS.md)

## Understand the implementation

- [Architecture](architecture/architecture.md) — components, state, and data flow
- [Implementation internals](architecture/internals.md) — file watching, sync, and agent mechanics
- [Storage](architecture/storage.md) — paths, persistence, and retention
- [Proposal internals](architecture/proposals.md) — capture, snapshots, and event history
- [Security model](architecture/security.md) — identity, membership, encryption, and limitations

## Integrate and operate

- [Local API](reference/api.md)
- [Daemon configuration](reference/daemon.md)
- [WebSocket and event protocols](reference/protocol.md)
- [Peer protocols](reference/p2p-protocols.md)
- [Invite formats](reference/invites.md)
- [Device linking protocol](reference/device-linking.md)
- [Agent execution inbox](reference/execution-inbox.md)
- [Agent integration details](reference/agent-integration.md)
- [Relay deployment guide](../guide/rendezvous-setup.md)

## Design proposals

[Design notes](designs/README.md) collect proposed work and open questions.
They may include shipped context to explain a proposal; use the guides and
references above for current behavior.

## Documentation conventions

Keep user tasks and observable behavior in `docs/guide/` and `docs/concepts/`.
Use `contributing/` for source builds and release procedures, `architecture/`
for implementation explanations, and `reference/` for APIs, protocols, and
integration details. Link to the relevant reference rather than repeating it in
user guides. Keep future plans in `designs/`, clearly separated from shipped
behavior, and update links whenever a page moves.
