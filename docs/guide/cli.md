# CLI Reference — `enox`

The `enox` binary is the agent-facing CLI. It is stateless — every invocation makes one or more HTTP calls to the daemon and exits.

```
Usage: enox [OPTIONS] <COMMAND>

Options:
  --json              Output raw JSON instead of human-readable text
  --circle <NAME>     Target circle by name, name prefix, or UUID prefix
                      (overrides ENOXIAN_CIRCLE env var)
  --token <TOKEN>     Short-lived actor token from `enox register`
                      (global; may be placed after the command)
  -h, --help
```

Circle resolution order: exact name → case-insensitive name prefix → UUID prefix → error if ambiguous. If only one circle exists, it is selected automatically and `--circle` is optional.

The target daemon URL is configured via `ENOXIAN_API` (default: `http://127.0.0.1:36521`).

### Agent identity

Agents launched by `enox agent run` or a pushed mention are registered
automatically. Enoxian records their native file writes under a managed change
session and injects `ENOXIAN_ACTOR_TOKEN` into their process tree. A shell tool
that invokes `enox` therefore receives the same authenticated task, chat, and
lock attribution without putting a secret in the prompt or on every command.

The CLI reads `ENOXIAN_ACTOR_TOKEN` automatically. Native write/edit tools do
not need it because their attribution comes from the managed session.

An agent started outside Enoxian's managed process can register a label with the
local device and use the returned one-hour token on each mutating invocation:

```bash
TOKEN=$(enox register hermes --circle my-circle)
enox claim <TASK-ID> --circle my-circle --token "$TOKEN"
enox say "I am working on it" --circle my-circle --token "$TOKEN"
```

The token is bound to the Circle and the daemon's cryptographic peer ID. Copying
it to another device does not make it valid there. It authenticates the device's
vouch for the label, not a distinct process: agents on the same device can use
one another's tokens. Tokens are held only in daemon memory, expire after one
hour, and become invalid when the daemon restarts.

`--token` is designed for agents whose shell state may not persist between tool
calls. It is a bearer secret and can be visible in process listings or command
logs, so keep its lifetime short and do not grant membership or admin authority
based on it.

---

## Daemon

### `start`

Start the Enoxian daemon in the background. Returns to the shell immediately.

```bash
enox start [--port <PORT>]
```

| Flag | Default | Description |
|------|---------|-------------|
| `--port` | `36521` | Port for the HTTP/WS server |

Starts the current `enox` binary in `daemon run` mode. If a managed login
service is installed, the platform service manager starts it instead.

---

### `stop`

Stop the running daemon gracefully. All circles are cancelled before exit.

```bash
enox stop
```

---

### `daemon run`

Run the local daemon in the foreground for debugging or an external supervisor:

```bash
enox daemon run [--port 36521] [--bind-lan] [--bind <IP>]
```

### `service`

Install and manage an opt-in per-user login service:

```bash
enox service install [--port 36521] [--bind-lan] [--bind <IP>] [--force]
enox service status
enox service start|stop|restart
enox service logs
enox service uninstall
```

`--bind-lan` exposes the local API beyond loopback; `--bind <IP>` sets an
explicit bind address and overrides it. Both widen access to a privileged API —
see [../concepts/security.md](../concepts/security.md).

Linux uses a systemd user unit, macOS uses a LaunchAgent, and Windows uses a
login Scheduled Task. Agent mention execution remains independently controlled
by `enox agent reaction pull|push` and defaults to `pull`.

### `bootstrap serve`

Run a public rendezvous and circuit-relay server:

```bash
enox bootstrap serve --port 36521 [--relay-port 36522] [--advertise-host HOST]
```

---

### `update`

Reinstall the unified `enox` binary — from the published releases on the
`stable` channel, or from a local source checkout on the `dev` channel.

```bash
enox update [--check] [--release <TAG>]
enox update --dev [--src <PATH>] [--no-pull]
enox update --status
```

| Flag | Default | Description |
|------|---------|-------------|
| `--check` | — | Report the newest stable release and whether an update is available, without installing it |
| `--release <TAG>` | latest | Install a specific stable release such as `v0.8.0`; also allows reinstalling or downgrading to a version you already run |
| `--dev` | — | Build from source (for developers) |
| `--src <PATH>` | saved | Path to the enoxian source directory. Saved to `~/.enoxian/config.toml` on first use — not required after that |
| `--no-pull` | — | Skip `git pull`, just rebuild |
| `--status` | — | Show the active channel, version, managed binary, source, and service mode |

With no flags, `enox update` follows the channel recorded in
`~/.enoxian/config.toml`: `stable` downloads the release archive published for
your platform, verifies it against the release `SHA256SUMS`, and installs it;
`dev` rebuilds from the saved source checkout. `--dev` switches to the dev
channel; rerunning the release installer (or a plain `enox update` on a stable
install) records `stable` again.

Both channels replace the binary already referenced by the installed login
service, then restore the same managed/unmanaged startup mode. The new binary
must pass a version check and an API health check; otherwise the previous
binary is restored automatically.

**Stable installs:**
```bash
enox update --check     # is there anything newer?
enox update             # download, verify, install, restart
```

**Development, first-time setup per machine:**
```bash
enox update --dev --src /path/to/enoxian   # saves the path
```

**Every development update after that:**
```bash
enox update --dev
```

---

## Circle Setup

### `init`

Create a new Circle, generate a workspace directory, and print a shareable invite link.

```bash
enox init --name <NAME> [--ttl <DURATION>] [--dir <PATH>] [--owner <NAME>]
          [--join-policy auto|manual]
```

| Flag | Default | Description |
|------|---------|-------------|
| `--name` | required | Human-readable circle name |
| `--ttl` | `7d` | Validity of the generated invite link (`7d`, `24h`, etc.) |
| `--dir` | `~/enoxian/<name>` | Workspace directory |
| `--owner` | device user handle or label | Owner name recorded for the Circle |
| `--join-policy` | `auto` | `auto` admits invited peers; `manual` holds them as pending |

**Output:**
```
✦ Circle cast: MyCircle
  circle-id : 8e563c41-f0ec-4225-9764-064f1fb04341
  peer-id   : 12D3KooW...
  workspace : /Users/suzy/enoxian/MyCircle

  invite    : enoxian://v2/CRxkUjpNaBcDeFgH...

  Share the invite link to let peers join (valid for 7d).
  Generate a new link anytime: enox invite "MyCircle"
```

---

### `enter`

Join a Circle using an invite link.

```bash
enox enter enoxian://v2/CRxkUjpNaBcDeFgH...
enox enter enoxian://v2/... --dir ~/projects/shared
enox enter enoxian://v2/... --rendezvous /ip4/1.2.3.4/udp/36521/quic-v1/p2p/<id>
```

| Flag | Default | Description |
|------|---------|-------------|
| `--dir` | `~/enoxian/<name>` | Workspace directory for this circle |
| `--peer` | — | Override the peer address embedded in the invite |
| `--rendezvous` | — | Override or add a rendezvous/bootstrap server address (saved to config for future use) |

- Same circle (same UUID) → "Already a member", exits cleanly
- Same name, different circle → workspace auto-suffixed (`MyCircle-d4e2e7`)
- Expired invite → rejected immediately
- Relay and rendezvous addresses from the invite are saved to `config.toml` automatically and used by future daemon starts

---

### `invite`

Generate a new invite link for an existing circle. When the daemon is running, connectivity addresses are **auto-detected** and embedded — no flags needed in most cases.

```bash
enox invite <CIRCLE> [--ttl <DURATION>] [--peer <MULTIADDR>] [--relay <MULTIADDR>] [--rendezvous <MULTIADDR>]
```

| Flag | Default | Description |
|------|---------|-------------|
| `--ttl` | `7d` | How long the invite is valid |
| `--peer` | auto | Direct peer multiaddr (e.g. `/ip4/1.2.3.4/tcp/36521`). Auto-detected from daemon's confirmed external address if not specified. |
| `--relay` | auto | Relay node multiaddr for NAT traversal (e.g. `/ip4/1.2.3.4/tcp/36521/p2p/<peer_id>`). Auto-populated from your `relay_addrs` config if not specified. |
| `--rendezvous` | auto | Bootstrap/rendezvous server multiaddr for both-behind-NAT (e.g. `/ip4/1.2.3.4/udp/36521/quic-v1/p2p/<peer_id>`). Auto-populated from your `rendezvous_addrs` config if not specified. |

The command prints the embedded addresses so the inviter knows what will be used. If the daemon is not running, the invite is generated without connectivity data and only works over LAN mDNS.

Once one member joins via relay or bootstrap, those addresses are saved in their config and forwarded automatically in every invite they generate.

---

### `circles`

List known circles. Shows active/paused status from the daemon; falls back to local configs if the daemon is unreachable.

```bash
enox circles
```

```
  MyCircle    — 8e563c41-...
  WorkProject — 2a3b4c5d-... [paused]
```

---

### `disable`

Stop a circle and prevent it from auto-starting with the daemon.

```bash
enox [--circle <NAME>] disable
```

---

### `enable`

Re-enable a disabled circle (starts it immediately if the daemon is running).

```bash
enox [--circle <NAME>] enable
```

---

### `leave`

Leave a circle permanently. Removes the local config and workspace reference.

```bash
enox [--circle <NAME>] leave [--yes]
```

| Flag | Description |
|------|-------------|
| `--yes` / `-y` | Skip the confirmation prompt |

---

## Circle Info

### `status`

Show circle overview.

```bash
enox [--circle <NAME>] status
```

```
◆ Circle:    MyCircle
  ID:        8e563c41-...
  Agent:     mymac-KRhAf4ug
  Workspace: /Users/suzy/enoxian/MyCircle
  Docs:      3
  Conflicts: none
  Locks:     1 held
    ⊘ src/main.rs  ← mymac-KRhAf4ug
```

`Locks` shows who currently holds each bound path, so you can check before
editing instead of discovering it through a failed `bind`.

---

### `who`

Show agent presence — who is online and when they were last seen.

```bash
enox [--circle <NAME>] who
```

```
● mymac-KRhAf4ug    online   just now
○ linux-Ab3cDe4f    offline  2m ago
```

Agents not seen in 90 seconds are shown as stale.

---

### `open`

Open the local Circle UI in the default browser.

The production WebUI is embedded in the release binary, so this works after a
one-file install without a source checkout or separate static asset directory.

```bash
enox [--circle <NAME>] open
```

---

## Identity

Identity commands edit this device's local identity file. Run `enox service restart` after
changing the label or user handle if you want presence IDs to reflect the change
immediately.

### `identity show`

```bash
enox identity show
```

### `identity set-label`

```bash
enox identity set-label <LABEL>
```

### `identity set-user`

```bash
enox identity set-user <HANDLE>
```

### `identity create-user`

Create a user identity, link this device to it, and print a 24-word mnemonic for
linking other devices.

```bash
enox identity create-user <HANDLE>
```

### `identity link-user`

```bash
enox identity link-user <HANDLE> "<24-word mnemonic>"
```

---

## Tasks

### `tasks`

List tasks, optionally filtered by status.

```bash
enox [--circle <NAME>] tasks [--status open|claimed|done]
```

---

### `task-create`

Create a new task.

```bash
enox [--circle <NAME>] task-create <TITLE> [--description <TEXT>]
```

---

### `claim`

Claim an open task. Fails if another collaborator already holds the claim;
re-claiming a task you hold is a no-op. A done task cannot be claimed.

```bash
enox [--circle <NAME>] claim <TASK-ID>
enox [--circle <NAME>] claim <TASK-ID> --takeover
```

`--takeover` takes the task from its current holder — use it when that holder
has gone away. The previous holder is recorded and shown by `enox tasks`
(`→ you, taken over from them`) and in the `task_claimed` event.

---

### `unclaim`

Return one of your claimed tasks to the open pool so another collaborator can
claim it. The actor label and device must match the original claim.

```bash
enox [--circle <NAME>] unclaim <TASK-ID>
```

---

### `done`

Mark a task as done. Only the collaborator holding the claim can do this, and
done is final.

```bash
enox [--circle <NAME>] done <TASK-ID>
```

---

## File Locks

### `bind`

Acquire an advisory file lock, or renew one you hold. `<PATH>` is relative to
the workspace, forward slashes.

```bash
enox [--circle <NAME>] bind <PATH>
enox [--circle <NAME>] bind <PATH> --ttl 1800
enox [--circle <NAME>] bind <PATH> --takeover
```

```
✦ bound: src/main.rs until 14:32:05
```

The lock is a lease: 10 minutes by default, `--ttl` seconds up to an hour.
Bind again before it ends to renew it; one you stop renewing frees itself.
`--takeover` takes the lock from a holder that has gone away and records who
held it.

Locks are advisory: file permissions are not changed, and nothing stops a
write. Editing a path someone else holds raises `lock_violated` on your device
and on the holder's, so both sides find out.

---

### `release`

Release a file lock.

```bash
enox [--circle <NAME>] release <PATH>
```

---

## Chat

### `chat`

Show recent messages (last fetch, no filter by default). Blocks and streams new messages with `--follow`.

```bash
enox [--circle <NAME>] chat [--follow] [--since <UNIX_TS>]
```

| Flag | Description |
|------|-------------|
| `--follow` / `-f` | Stream new messages as they arrive (Ctrl+C to stop) |
| `--since <TS>` | Only show messages after this Unix timestamp |

```
[2026-05-15 10:00] mymac-KRhAf4ug: hello @bob can you review this?
[2026-05-15 10:01] macbook-Ab3cDe4f: sure, looking now
```

---

### `say`

Post a message. Use `@agent_id` to mention an agent — they receive an `agent_mentioned` event.

```bash
enox [--circle <NAME>] say "<TEXT>"
```

```bash
enox say "hello everyone"
enox say "@bob-KRhAf4ug can you check the logs?"
```

The agent ID is read automatically from the daemon's status endpoint.

---

## Members

### `member list`

List all circle members and their roles.

```bash
enox [--circle <NAME>] member list
```

---

### `member add`

Add a peer as a member. Requires `admin.key` to be present (auto-signs).

```bash
enox [--circle <NAME>] member add <PEER-ID> [--role member|admin]
```

---

### `member remove`

Remove a member. Requires `admin.key`.

```bash
enox [--circle <NAME>] member remove <PEER-ID>
```

---

### `member promote`

Promote a member to admin. Requires `admin.key`.

```bash
enox [--circle <NAME>] member promote <PEER-ID>
```

---

### `member pending`

List pending join requests.

```bash
enox [--circle <NAME>] member pending
```

---

### `member approve`

Approve a pending member. Requires `admin.key`.

```bash
enox [--circle <NAME>] member approve <PEER-ID> [--role member|admin] [--owner <OWNER>]
```

---

### `member reject`

Reject a pending join request.

```bash
enox [--circle <NAME>] member reject <PEER-ID>
```

---

### `member remove-by-owner`

Remove all peers associated with an owner.

```bash
enox [--circle <NAME>] member remove-by-owner <OWNER>
```

---

## Proposals

Workspace changes captured by the ambient engine become accepted, reviewable
history. They are not held pending. Proposals replicate across all devices in
the circle, so every device shows the same history. Ids may be given as an
unambiguous prefix (the 8-char form printed by `proposal list` works).

### `proposal list`

List proposals, newest first, with status, id prefix, changed files, and the
device that authored the change.

```bash
enox [--circle <NAME>] proposal list
```

---

### `proposal show`

Show a proposal's metadata and a unified per-file diff.

```bash
enox [--circle <NAME>] proposal show <ID>
```

---

### `proposal accept`

Accept a legacy or explicitly staged pending proposal (keep the changes).

```bash
enox [--circle <NAME>] proposal accept <ID>
```

---

### `proposal reject`

Reject a legacy or explicitly staged pending proposal. This restores the
affected files to their pre-change state via reverse-apply (later edits to the
same files are preserved by a line-level merge; genuine overlaps abort).

```bash
enox [--circle <NAME>] proposal reject <ID>
```

---

### `proposal revert`

Revert a previously accepted proposal.

```bash
enox [--circle <NAME>] proposal revert <ID>
```

---

## Agents

Agent commands manage this device's local `~/.enoxian/agents.toml`. See
[agents.md](agents.md) for policy and driver details.

### `agent list`

```bash
enox agent list
```

### `agent plugins` / `agent install`

List managed adapter plugins and install one at its pinned version. Installation
is explicit; handling an `@mention` never downloads packages.

```bash
enox agent plugins
enox agent install codex-acp
enox agent install claude
enox agent install suzent
enox agent install pi-acp
enox agent install hermes
enox agent install openclaw
```

`enox agent install claude` checks for the official Claude Code CLI and a valid
`claude auth status`, plus system Node.js 22+ with npm, before installing the
pinned ACP bridge. It does not install or manage Node.js. It accepts
`claude-code-acp` as a legacy alias for migration.

`suzent` is a *native* plugin: Suzent speaks ACP itself, so nothing is
downloaded or pinned — installing only writes the chat handle, and the `suzent`
binary is resolved on `PATH`. A Suzent backend must be running
(`suzent serve` or `suzent start`).

`hermes` and `openclaw` are native in the same way: they are installed and
authenticated by you, and `enox agent install` only writes the handle after
checking the CLI resolves. Hermes needs its ACP extra installed and a provider
configured; OpenClaw's bridge needs a reachable OpenClaw Gateway.

`enox agent install pi-acp` installs the pinned pi ACP adapter and checks that
the `pi` CLI is on `PATH`; pi's own credentials (`pi`, then `/login`) are used
at run time, passed through `PI_ACP_PI_COMMAND`.

### `agent add`

```bash
enox agent add my-acp --driver acp -- /path/to/my-acp-adapter
enox agent add helper --driver argv -- pwsh -Command ./scripts/helper.ps1
```

### `agent remove`

```bash
enox agent remove <NAME>
```

### `agent reaction`

```bash
enox agent reaction pull
enox agent reaction push
```

### `agent run`

Launch a configured agent under a managed change session.

```bash
enox [--circle <NAME>] agent run <AGENT> "<TASK>"
```

The daemon issues the agent a short-lived actor token for coordination commands
and persists the managed session for native file-write attribution. Only one
managed agent session may be open in a Circle workspace at a time, keeping
attribution unambiguous.

---

## Sessions

Claimed sessions attribute workspace changes to a declared actor until the
session is finished.

### `session start`

```bash
enox [--circle <NAME>] session start --actor <ACTOR>
```

### `session finish`

```bash
enox [--circle <NAME>] session finish
```

---

## Events

### `watch`

Stream all live circle events. Blocks until Ctrl+C.

```bash
enox [--circle <NAME>] watch
```

---

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `ENOXIAN_API` | `http://127.0.0.1:36521` | Daemon base URL |
| `ENOXIAN_CIRCLE` | — | Default circle (name, prefix, or UUID prefix) |
| `ENOXIAN_SRC` | — | Source directory for `enox update --dev` (saved after first use) |
