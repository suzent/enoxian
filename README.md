# enoxian

**A P2P collaboration layer for humans, AI agents, and scripts working in the
same directory.**

enoxian gives a project a shared **Circle**: files sync in real time, people and
agents coordinate through tasks and chat, risky edits can be locked, and
agent-made changes become reviewable proposals instead of mysterious local
diffs.

> "Descend not to the API layer to unify agents. Descend to the protocol layer."

---

## What You Get

- **Real-time file sync** over a Yjs CRDT document layer, with offline-friendly
  merge semantics.
- **Circle coordination**: task board, chat, presence, file locks, and live
  event streaming.
- **Agent execution**: mention configured local agents, run them directly with
  `enox agent run`, and capture their file changes as proposals.
- **Reviewable workspace changes**: accept, reject, or revert attributed
  proposals from agents, scripts, and claimed sessions.
- **P2P membership**: invite links, LAN discovery, relay/rendezvous WAN
  bootstrap, admin-signed members, and MLS-backed removal state.
- **Local-first API**: the managed Enoxian daemon exposes a loopback
  HTTP/SSE/WebSocket API used by
  the CLI, web UI, and automation.

---

## Install

### Linux / macOS

```sh
curl -fsSL https://github.com/suzent/enoxian/releases/latest/download/install.sh | sh
```

The installer chooses the matching Linux/macOS archive and installs to a
writable user location without requiring `sudo`. To pin a version or location:

```sh
curl -fsSL https://github.com/suzent/enoxian/releases/latest/download/install.sh | \
  sh -s -- --version v0.3.4 --bin-dir "$HOME/.local/bin"
```

### Windows PowerShell

```powershell
irm https://github.com/suzent/enoxian/releases/latest/download/install.ps1 | iex
```

PowerShell installs to `%LOCALAPPDATA%\enoxian\bin` and adds it to the user
`PATH`. To pin a version, set `$env:ENOXIAN_VERSION = 'v0.3.4'` first.

To keep Enoxian available after login, opt into the per-user service during
installation (`--enable-service` on Linux/macOS or `-EnableService` on Windows),
or run `enox service install` later. Agent mention execution remains disabled
until the user explicitly selects `enox agent reaction push`.

Release installers verify checksums and the downloaded binary before making
an atomic, rollback-protected replacement. See
[docs/guide/releasing.md](docs/guide/releasing.md) for the release process.

### From Source

```sh
git clone https://github.com/suzent/enoxian
cd enoxian
cargo build
```

The source build creates one `target/debug/enox` binary. CLI commands are
short-lived, while `enox daemon run` is the foreground daemon used internally
by `enox start` and managed login services.

Rust 1.88 or newer is required. Node.js is only needed when building the
frontend in release mode.

---

## Quick Start

Create a Circle:

```sh
enox init --name my-project
```

Start the daemon:

```sh
enox start
```

Or install a login-time service with crash recovery:

```sh
enox service install
enox service status
```

Use the CLI:

```sh
enox status
enox tasks
enox who
enox watch
```

Open the embedded WebUI without installing a separate web server or asset
directory:

```sh
enox open
```

With multiple local circles, pass `--circle` or set `ENOXIAN_CIRCLE`:

```sh
enox --circle my-project status
```

Invite another machine or agent host:

```sh
enox invite my-project
enox enter enoxian://v1/...
```

See [docs/guide/getting-started.md](docs/guide/getting-started.md) for the
full setup flow, including source installs, daemon logs, WAN connectivity, and
multi-circle behavior.

---

## Daily Workflow

```sh
# Coordinate work
enox task-create "write sync tests" --description "cover lock arbitration"
enox tasks
enox claim <task-id>
enox done <task-id>

# Avoid conflicts on high-risk files
enox bind src/main.rs
enox release src/main.rs

# Talk in the Circle
enox say "can someone review the proposal?"
enox chat -f

# Review captured workspace changes
enox proposal list
enox proposal show <proposal-id>
enox proposal accept <proposal-id>
enox proposal reject <proposal-id>
```

Configure a local AI agent with a pinned managed adapter:

```sh
# Requires the official Claude Code CLI, `claude auth login`, and system
# Node.js 22+ with npm.
enox agent install claude
enox agent reaction push
enox say "@claude add tests for the invite parser"
```

Or point at an agent that already speaks ACP itself — no adapter, no Node.js:

```sh
enox agent add suzent --driver acp -- suzent acp
enox say "@suzent add tests for the invite parser"
```

Or run one directly:

```sh
enox agent run claude "summarize the proposal store API"
```

Agent configuration is device-local in `~/.enoxian/agents.toml`; it is not
synced to the Circle. See [docs/guide/agents.md](docs/guide/agents.md) for ACP,
argv fallback agents, mention targeting, and session memory.

---

## How It Works

```
┌─────────────────────────────────┐
│        Application Layer        │  humans, agents, scripts, web UI
├─────────────────────────────────┤
│      Coordination Layer         │  tasks, chat, locks, presence, proposals
├─────────────────────────────────┤
│      Document Sync Layer        │  Yjs CRDT file and control documents
├─────────────────────────────────┤
│      Transport Layer            │  libp2p Noise + Yamux, relay/DCUtR
├─────────────────────────────────┤
│      Discovery Layer            │  mDNS, Kademlia, rendezvous
└─────────────────────────────────┘
```

Every participant runs one Enoxian daemon for all enabled Circles. Editors and agents
use normal filesystem IO; the daemon watches files, syncs CRDT updates to peers,
and serves the local API. `enox` is a thin CLI client over that API.

For a fuller walkthrough, start with
[docs/concepts/overview.md](docs/concepts/overview.md).

---

## Current Status

The current package version is **0.4.2**.

| Area | Status |
|------|--------|
| P2P circles, file sync, tasks, chat, presence, locks | Complete |
| CLI, local API, SSE/WebSocket event stream | Complete |
| WAN invites, relay/rendezvous bootstrap | Complete |
| Members, identity, MLS membership/removal gate | Complete |
| MLS-derived P2P content encryption | Complete |
| Proposal capture, review, reject, revert | Complete |
| ACP/argv agent execution, mention reactions, agent memory | Complete |
| Cross-platform packaging and install scripts | Complete |
| At-rest encryption for persisted workspace/control data | Planned |

See [CHANGELOG.md](CHANGELOG.md) for release notes and the
[documentation index](docs/index.md) for current behavior.

---

## Documentation

| Start Here | Description |
|------------|-------------|
| [docs/guide/getting-started.md](docs/guide/getting-started.md) | Install, create a Circle, run the daemon, join peers |
| [docs/guide/cli.md](docs/guide/cli.md) | Complete `enox` command reference |
| [docs/guide/agents.md](docs/guide/agents.md) | Configure ACP/argv agents and mention reactions |
| [docs/guide/invite.md](docs/guide/invite.md) | Invite links, TTLs, relay and rendezvous hints |
| [docs/reference/api.md](docs/reference/api.md) | Local REST/SSE/WebSocket API |
| [docs/concepts/security.md](docs/concepts/security.md) | Trust model, PSK, MLS membership, data-at-rest notes |
| [docs/concepts/architecture.md](docs/concepts/architecture.md) | Components, state model, directory layout |

The full index lives at [docs/index.md](docs/index.md). Agent collaboration
rules for this repository live in [AGENTS.md](AGENTS.md).

---

## Tech Stack

| Crate | Role |
|-------|------|
| `tokio` | Async runtime |
| `libp2p` | P2P transport, discovery, relay, rendezvous |
| `yrs` | Yjs CRDTs |
| `axum` | HTTP, SSE, and WebSocket server |
| `notify` | Cross-platform file watcher |
| `reqwest` | CLI HTTP client |
| `clap` | CLI argument parsing |
| `openmls` | MLS membership state |

## License

enoxian is available under the [MIT License](LICENSE).
