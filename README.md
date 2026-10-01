# enoxian

**A shared workspace for people and AI agents, across devices.**

enoxian brings shared notes, conversations, tasks, and AI agents into a
**Circle**. Each device keeps a local copy of the shared folder, so you can
edit with your usual tools and pick up work on another machine.

- **Keep context together.** Share notes, decisions, and hand-offs across devices.
- **Coordinate work.** See who is online, claim tasks, and reserve files while editing.
- **Work with agents.** Mention a configured agent in chat and follow its progress.
- **Review changes.** Inspect file history and undo changes with proposals.
- **Work locally.** Files remain available offline and sync when devices reconnect.

## Install

**macOS / Linux**

```sh
curl -fsSL https://github.com/suzent/enoxian/releases/latest/download/install.sh | sh
```

**Windows PowerShell**

```powershell
irm https://github.com/suzent/enoxian/releases/latest/download/install.ps1 | iex
```

## Create your first Circle

```sh
enox init --name my-circle
enox start
enox open
```

Open the shared folder shown by `enox init` and add a note with your usual
editor. Use the web UI to chat, view tasks, and inspect file history.

The Circle folder is for shared knowledge: notes, decisions, hand-offs, and
pointers to work. Keep code repositories, dependencies, and build output in
separate local checkouts.

To invite someone, generate a link and share it privately:

```sh
enox invite my-circle
```

On their device, after installing enoxian:

```sh
enox enter "<invite-link>"
enox start
enox open
```

For your own second device, use [device linking](docs/guide/link.md) to carry
over your identity and join your Circles.

The [getting started guide](docs/guide/getting-started.md) covers installation
options, login services, multiple Circles, and connection troubleshooting.

## Bring in an agent

For example, with Claude Code installed and authenticated, plus Node.js 22+
and npm:

```sh
enox agent install claude
enox agent reaction push
enox say "@claude summarize the notes and suggest next steps"
```

Enabling `push` allows Circle mentions to launch configured agents on this
device. Agent settings stay local to each device. See the
[agent guide](docs/guide/agents.md) for supported agents, permissions, and
conversation settings.

File changes normally reach the live folder immediately. Proposals let you
inspect and undo those changes; see [reviewing changes](docs/concepts/proposals.md).

## Documentation

- [Getting started](docs/guide/getting-started.md)
- [Everyday collaboration](docs/guide/collaboration.md)
- [Inviting people](docs/guide/invite.md) and [linking your devices](docs/guide/link.md)
- [Using agents](docs/guide/agents.md)
- [Privacy and security](docs/concepts/security.md)
- [CLI reference](docs/guide/cli.md)

Browse the [documentation index](docs/index.md) for all user guides.
For contributing, building from source, integrations, or architecture, use the
[developer documentation](docs/development/README.md).
Release notes are in the [changelog](CHANGELOG.md).

## License

enoxian is available under the [MIT License](LICENSE).
