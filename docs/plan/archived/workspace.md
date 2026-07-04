# Workspace Folders

**Status:** Complete. This M1 design has been implemented and summarized in
[milestones.md](milestones.md#m1--workspace-folders).

Current authoritative references:

- Core concepts: [../../concepts.md](../../concepts/concepts.md)
- Getting started: [../../getting-started.md](../../guide/getting-started.md)
- CLI commands: [../../cli.md](../../guide/cli.md)
- Daemon configuration: [../../daemon.md](../../reference/daemon.md)

## What Shipped

- Visible per-circle workspace directories.
- Default workspace path: `~/enoxian/<circle-name>/`.
- `--dir` support for `enox init`.
- `--dir` support for `enox enter`.
- `workspace_dir` stored in `config.toml`.
- Local name conflict handling for same-name circles.
- Workspace path shown in status output.

## Semantics

The hidden config directory and the visible workspace are separate:

```text
~/.enoxian/circles/<circle-id>/config.toml   # credentials and config
~/enoxian/<circle-name>/                     # user-visible files
```

The workspace can be any directory chosen by the user. The daemon watches that
directory and syncs file changes through the circle.
