# Circle Lifecycle

**Status:** Complete. This M4 design has been implemented and summarized in
[milestones.md](milestones.md#m4--circle-lifecycle).

Current authoritative references:

- CLI commands: [../../cli.md](../../guide/cli.md)
- Daemon behavior: [../../daemon.md](../../reference/daemon.md)
- API endpoints: [../../api.md](../../reference/api.md)
- Current roadmap: [../roadmap.md](../roadmap.md)

## What Shipped

- `disabled` flag in circle config.
- `enox disable` and `enox enable`.
- `enox leave`.
- Runtime `POST /circles/<id>/stop`.
- Runtime `POST /circles/<id>/start`.
- Per-circle cancellation tokens.
- Hot-reload for newly enabled circles.
- `enox circles` shows paused circles.

## Semantics

Disable is a local pause:

```text
config remains
workspace remains
circle can be re-enabled
other peers are unaffected
```

Leave is a local removal:

```text
local config is removed
workspace files remain
other peers are unaffected
rejoin requires a new invite
```

Workspace files are never deleted by lifecycle commands.
