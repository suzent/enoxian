# Changelog

All notable changes to enoxian are recorded here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

<!--
## How to update this changelog

Rules (keep them so releases stay clean — the release notes are built from this
file, see below):

1. **Every user-facing change adds a line under `## [Unreleased]`.** Put it in
   the right group: **Added**, **Changed**, **Deprecated**, **Removed**,
   **Fixed**, or **Security**. Omit empty groups.
2. **Write for users, not commits.** One line per change, describing the effect
   ("Restarting the daemon no longer re-triggers past mentions"), not the diff.
   Bundle several commits into one entry when they serve one change.
3. **Skip pure-internal churn** — refactors, test-only changes, docs typos, CI
   tweaks that users never see. If a user can't observe it, it doesn't belong.
4. **Security-relevant changes always go under `Security`,** even small ones.
5. **At release time, cut a version section:** rename `[Unreleased]` you were
   filling to `## [x.y.z] — YYYY-MM-DD`, add a fresh empty `## [Unreleased]`
   above it, and update the compare links at the bottom. (`scripts/bump.sh` does
   not do this yet — it is a manual step.)
6. **Versioning:** breaking change → major; new feature → minor; fix only →
   patch (pre-1.0, minor also absorbs features that aren't clearly breaking).

## How release notes are built

On a tagged release, `.github/workflows/release.yml` uses the matching version
section from THIS file as the top of the GitHub release notes, followed by the
auto-generated commit/PR list. So: curated summary here, full commit list
appended automatically. Keep the section for a version accurate before tagging.
-->


## [Unreleased]

## [0.3.4] — 2026-08-18

### Fixed

- Release binaries now embed the production WebUI, so `enox open` and `/app`
  work after a one-file install without requiring a source checkout or separate
  static asset directory. Release CI exercises both the HTML entry point and a
  hashed JavaScript asset on Linux and Windows.

## [0.3.3] — 2026-08-18

### Changed

- Development updates now replace the binary already owned by the login service
  instead of creating a competing `~/.cargo/bin` installation. Updates preserve
  managed/unmanaged startup mode, verify API health, roll back failed swaps, and
  expose channel details through `enox update --status`.

## [0.3.2] — 2026-08-17

### Fixed

- Windows login-service stop, restart, and forced reinstall now clean up daemon
  processes left by Task Scheduler's logging wrapper, reliably releasing the
  API port while preserving `enox service logs`.

## [0.3.1] — 2026-08-17

### Fixed

- Windows login-service installation now writes Task Scheduler XML as UTF-16LE
  with a BOM and automatically recovers from definitions left by a failed
  registration, avoiding the localized “cannot switch encoding” error.

## [0.3.0] — 2026-08-17

### Added

- Added `enox service install|status|start|stop|restart|logs|uninstall` for
  opt-in login-time startup through systemd user units, macOS LaunchAgents, and
  Windows Scheduled Tasks.
- Release publication now runs the published one-click installer on clean
  Linux, macOS, and Windows runners before the release is considered validated.
- Installers can enable login-time startup explicitly with `--enable-service`
  or `-EnableService`; Agent mention execution remains a separate opt-in.

### Changed

- Enoxian now ships one `enox` executable. `enox start` launches the same binary
  in background daemon mode, while `enox daemon run` provides a foreground mode
  for debugging and external supervisors.
- Public rendezvous and relay deployments now use `enox bootstrap serve`, and
  the VPS scripts migrate the old binary and systemd unit automatically.
- Background startup writes persistent logs and uses crash-restart policies
  without exposing the privileged local API beyond loopback by default.

### Removed

- Removed the standalone `enoxd` executable and its duplicated packaging,
  update, installation, and documentation paths.

## [0.2.1] — 2026-08-16

### Added

- Proposal sync now fetches missing content-addressed blobs after proposal
  manifests arrive, so large proposal files omitted from bundles can become
  reviewable and revertible on other peers.
- Release binaries now expose `--version`, and installers verify published
  SHA256 checksums before replacing binaries.
- Release automation now gates tags on version/CHANGELOG consistency, Rust and
  frontend checks, builds all platform artifacts before publishing, and can
  update an optional Homebrew tap.
- One-click installers now select a user-writable location, support pinned
  versions and custom destinations, test binaries before replacement, roll
  back failed upgrades, and give actionable PATH and daemon guidance.

### Changed

- Release workflows now use immutable Action commits, least-privilege job
  permissions, and Node.js 22; dependency updates are monitored by Dependabot.

### Security

- Updated Rust networking/runtime transitive dependencies and the frontend
  build toolchain to versions containing upstream security fixes.
- Added a private vulnerability reporting policy and contributor guidance for
  keeping credentials and local Circle state out of commits.

## [0.2.0] — 2026-07-05

### Added

- **Agent execution over the Agent Client Protocol (ACP).** Mention `@claude`
  (or any configured agent) in a circle's chat and, under a per-device *push*
  policy, the daemon runs it in the workspace; its file changes become reviewable
  proposals and its reply is posted back to chat. Also runnable directly with
  `enox agent run`. Includes an `argv` fallback driver for non-ACP tools.
- **Hierarchical mentions** — `@owner`, `@owner/device`, `@owner/device/agent` —
  with a `@` autocomplete over the member tree and atomic mention chips in chat.
- **Agent session memory** — a persistent ACP session per (circle, agent),
  resumed on the next mention, with cold-start context (brief + recent chat)
  injected only for fresh/recovered sessions.
- **Configure agents from the CLI and frontend** — `enox agent add/remove/list`,
  `enox agent reaction push|pull`, and an editable Device Settings panel.
- **Document-aware proposal diffs (M16)** — text, markdown (per-section), JSON
  (object-path), code (function/class-level), and binary adapters, with
  formatter-noise detection. Surfaced in the proposal detail API.
- **Control-doc persistence (M14.5)** — chat (last 30 days), tasks, and the
  member list now survive an all-offline restart; presence is never persisted.
- **Local API authentication (M13)** — the daemon HTTP/WS API now requires a
  local token, binds to loopback by default (`--bind-lan` / `--bind` to widen),
  and restricts CORS to local origins.
- **Packaging (M18)** — CI across Linux/macOS/Windows, cross-platform release
  binaries, a bootstrap Docker image, `scripts/install.sh` / `install.ps1`, and a
  Homebrew formula (auto-updated by the release workflow).
- Documentation reorganized into `docs/guide`, `docs/reference`, `docs/concepts`;
  new guides for driving agents, agent memory, and control persistence.

### Fixed

- Restarting the daemon no longer re-triggers every past chat mention (durable
  dedup + a freshness cutoff); an agent's reply never wakes another agent.
- Agent chat replies post the agent's final message instead of a concatenation
  of every streamed message (no more run-together greetings).
- Fresh-session prompts fence the injected context so the agent responds only to
  the request, not to the background brief.
- The Vite dev proxy authenticates against the hardened API.
- Windows agent spawning (`npx` and other `.cmd` launchers) and process-tree
  cleanup.

### Security

- Chat, tasks, and members are persisted **plaintext at rest** (pre-M17 content
  encryption). See `docs/concepts/security.md` → Data At Rest.

---

## [0.1.4]

Baseline release prior to the agent-execution and packaging work above. See the
git history and `docs/plan/archived/milestones.md` for the M1–M14 feature set
(P2P sync, presence/tasks/locks/chat, members + MLS membership, WAN bootstrap,
and the local workspace proposal layer).

[Unreleased]: https://github.com/suzent/enoxian/compare/v0.3.4...HEAD
[0.3.4]: https://github.com/suzent/enoxian/compare/v0.3.3...v0.3.4
[0.3.3]: https://github.com/suzent/enoxian/compare/v0.3.2...v0.3.3
[0.3.2]: https://github.com/suzent/enoxian/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/suzent/enoxian/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/suzent/enoxian/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/suzent/enoxian/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/suzent/enoxian/compare/v0.1.4...v0.2.0
[0.1.4]: https://github.com/suzent/enoxian/releases/tag/v0.1.4
