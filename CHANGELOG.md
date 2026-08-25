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
5. **At release time, cut a version section:** the "Prepare release" workflow
   (Actions tab) runs `scripts/bump.sh`, which adds a dated version heading
   below a fresh empty `[Unreleased]` section and updates the compare links. It
   refuses to run when `[Unreleased]` is empty.
6. **Versioning:** breaking change → major; new feature → minor; fix only →
   patch (pre-1.0, minor also absorbs features that aren't clearly breaking).

## How release notes are built

On a tagged release, `.github/workflows/release.yml` uses the matching version
section from THIS file as the top of the GitHub release notes, followed by
GitHub's auto-generated commit/PR list and the install instructions. So: curated
summary here, full commit list appended automatically. Keep the section for a
version accurate before merging the release pull request — the release pipeline
refuses to publish a version whose section is missing or empty.
-->


## [Unreleased]

### Changed

- The WebUI now provides responsive, collapsible workspace panels; consistent
  file browsing, editing, and safe Markdown/HTML previews; clearer task and
  device information; and smoother chat, file, and Circle transitions.

## [0.4.3] — 2026-08-25

### Added

- Suzent can be driven as an agent with no adapter plugin and no Node.js: it
  speaks ACP itself, so `enox agent add suzent --driver acp -- suzent acp` is
  the whole setup and `@suzent` then works like `@claude`. The turn runs on your
  own Suzent install, with its memory, skills, and model configuration, in the
  circle workspace.
- Custom agents in Device Settings now show whether they can actually start —
  **READY**, **MISSING** with the command that could not be found, or
  **DOWNLOADS** for a `npx …` command — plus a description for agents Enoxian
  knows. Previously a custom entry showed only its command line, so a typo or an
  uninstalled CLI looked identical to a working agent.

### Fixed

- A device no longer advertises an agent whose own command is missing. An agent
  that speaks ACP itself names its product CLI directly rather than an adapter,
  so nothing caught it being absent: peers were offered the agent, the mention
  popup marked it runnable, and the failure surfaced only after someone
  addressed it.

### Security

- Release archives now carry signed, transparency-logged build provenance tying
  each archive to this repository, workflow, and commit. Verify a download with
  `gh attestation verify enoxian-macos-aarch64.tar.gz --repo suzent/enoxian`.
  The release pipeline verifies the published archives before a release is
  marked latest.

### Changed

- `@codex` now runs the Codex CLI you installed and signed in to, the same way
  `@claude` already used your Claude Code CLI, instead of a copy bundled inside
  the adapter. Device Settings reports **codex CLI missing** with install and
  login guidance when that CLI is absent, rather than showing the adapter as
  ready, and each ready adapter now states which CLI it runs.

### Fixed

- Agent replies in chat are attributed to the device that actually ran the
  agent. When two devices configured the same agent name, a reply could be
  shown under the wrong device.
- A device no longer advertises an agent whose CLI is not installed. Mention
  autocomplete offered such an agent as runnable, and the failure only appeared
  after someone addressed it. Installing the missing CLI restores the agent on
  the next daemon start.
- An open Circle now picks up membership changes made on another device —
  including the agents a device advertises — instead of showing the roster as it
  was when the Circle was opened. Peers going offline and coming back update
  live too. Previously both needed a page reload.

## [0.4.2] — 2026-08-24

### Fixed

- The managed login service (launchd on macOS, `systemd --user` on Linux)
  starts with a bare `PATH` and never sourced shell rc files, so Node.js and
  agent CLIs installed via a version manager like nvm (rather than a
  system-wide location) were invisible to the daemon even though they worked
  in any terminal — agent adapters wrongly reported "Node.js 22+ required" or
  the CLI as missing. The daemon now resolves the same `PATH` a login shell
  would and adopts it at startup, so adapter detection matches what's
  actually installed.
## [0.4.1] — 2026-08-23

### Fixed

- Circle sync and WebUI requests no longer block daemon worker threads while a
  CRDT document is busy. Contended requests return a retryable response, and the
  WebUI now times out with a visible **Try again** action instead of loading
  forever.
- Daemon shutdown now cancels circle tasks and long-lived WebSocket/SSE streams,
  enforces a bounded graceful-drain period, and times out unresponsive stop
  requests instead of leaving the API port wedged. The control API also starts
  before circle workspace loading, so slow startup cannot block stop or update
  commands. `enox stop` also stops the managed service when one is installed,
  rather than allowing its supervisor to bring the daemon back.
- Stable installers and development updates now bound calls into an older
  binary, terminate orphaned daemon processes when needed, and preserve an
  existing managed service across upgrades, allowing affected 0.4.0 installs to
  update without manual process cleanup.

## [0.4.0] — 2026-08-22

### Added

- Workspace changes now produce a causally ordered, peer-synchronized event
  history with deterministic materialization of proposal decisions, merges,
  conflicts, and the current frontier.

### Changed

- Normal human, agent, script, and remote workspace edits now land immediately
  as accepted, revertible proposal history instead of appearing behind a
  misleading pending-review gate. Pending proposals remain supported only for
  legacy records and explicitly isolated workflows.

### Security

- CRDT, proposal, and workspace-event payloads now use authenticated
  ChaCha20-Poly1305 frames with purpose-specific keys derived from the active MLS
  epoch. Membership bootstrap and commit replay let retained offline members
  recover current keys while removed members cannot derive future epoch keys.

## [0.3.8] — 2026-08-21

### Added

- Chat now shows short-lived typing and working indicators for people and
  agents, so participants can tell when a request has been seen and is being
  processed without waiting for the final response.

### Fixed

- Claude Code agent installation now uses the maintained `claude-agent-acp`
  bridge, requires the real Claude Code CLI and its authenticated session, and
  preserves native Claude settings by passing the resolved CLI executable to
  the bridge. Adapter installation now preflights system Node.js 22+ and npm
  with actionable CLI and Device Settings guidance. Existing
  `claude-code-acp` configurations remain migratable.

## [0.3.7] — 2026-08-19

### Fixed

- Windows login startup now uses a windowless WScript launcher instead of a
  PowerShell-to-`cmd.exe` console chain. `enox start` no longer opens a command
  window, and existing 0.3.5/0.3.6 service definitions migrate automatically.

## [0.3.6] — 2026-08-18

### Fixed

- The Windows installer now stops and waits for an existing managed service
  before replacing `enox.exe`, then restarts it automatically. Failed upgrades
  also restore the previous service instead of leaving Enoxian stopped.

## [0.3.5] — 2026-08-18

### Fixed

- On Windows, `enox start` and the login service now run behind a hidden managed
  process instead of a visible `cmd.exe` window, so closing a terminal no longer
  stops Enoxian. Existing managed services migrate automatically on the next
  start or update.

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

Baseline release prior to the agent-execution and packaging work above. The
M1–M14 feature set covered P2P sync, presence/tasks/locks/chat, members and MLS
membership, WAN bootstrap, and the local workspace proposal layer.

[Unreleased]: https://github.com/suzent/enoxian/compare/v0.4.3...HEAD
[0.4.3]: https://github.com/suzent/enoxian/compare/v0.4.2...v0.4.3
[0.4.2]: https://github.com/suzent/enoxian/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/suzent/enoxian/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/suzent/enoxian/compare/v0.3.8...v0.4.0
[0.3.8]: https://github.com/suzent/enoxian/compare/v0.3.7...v0.3.8
[0.3.7]: https://github.com/suzent/enoxian/compare/v0.3.6...v0.3.7
[0.3.6]: https://github.com/suzent/enoxian/compare/v0.3.5...v0.3.6
[0.3.5]: https://github.com/suzent/enoxian/compare/v0.3.4...v0.3.5
[0.3.4]: https://github.com/suzent/enoxian/compare/v0.3.3...v0.3.4
[0.3.3]: https://github.com/suzent/enoxian/compare/v0.3.2...v0.3.3
[0.3.2]: https://github.com/suzent/enoxian/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/suzent/enoxian/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/suzent/enoxian/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/suzent/enoxian/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/suzent/enoxian/compare/v0.1.4...v0.2.0
[0.1.4]: https://github.com/suzent/enoxian/releases/tag/v0.1.4
