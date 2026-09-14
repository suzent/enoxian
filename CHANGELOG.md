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

### Added

- `enox link` puts your identity on a second device without the 24-word
  mnemonic. Run it on the machine you already use, type the four words it prints
  on the new one, check that both screens show the same six-digit number, and
  the new device is linked with every circle already joined. The user root key
  never leaves the device that holds it — the new machine generates its own
  device key and receives a signature over it — so a linked device can later be
  removed on its own.

- `enox identity show` now says whether a device's attestation actually
  verifies, rather than only that one is present.

### Fixed

- The daemon no longer sometimes refuses to start with "execution inbox already
  has an active owner" right after a restart. It could lose a race against its
  own previous instance's release of the inbox lock, because a concurrently
  launched agent process briefly inherits that lock; the daemon now waits the
  moment out instead of giving up.

- Renaming a device with `enox identity set-label` no longer erases the stored
  recovery phrase. Any save of the identity file used to drop it, so a rename —
  or receiving a link — silently destroyed the only copy of the user root key.

- Renaming a device no longer invalidates its attestation. The device label was
  part of what the attestation signed, so a rename permanently broke it with no
  way to reissue one.

### Security

- `enox link` and rendezvous address resolution no longer send the local
  daemon's API token to the remote bootstrap server. Both used the CLI's shared
  HTTP client, which carries that token as a default header, so talking to a
  pairing or bootstrap host disclosed a privileged local credential over plain
  HTTP.

### Changed

- Read-the-room agents now take turns fairly. Choose a listener count per message,
  or cycle the count from one up to that limit. Selected requests use the shared
  execution queue instead of favoring the first configured agent.
- Agent activity shows device names, original messages and plain-language outcomes;
  completed runs are collapsed in the sidebar and legacy import records are hidden.


- Follow-ups now use explicit reply threads by default. Configurations that omit
  `engagement_window_secs` now use `0` instead of `180`; set it to `180` in Device
  Settings to keep the previous three-minute recency routing. Explicitly configured
  windows are preserved.

- Invite links are much shorter. A typical `enox invite` link is now around 300
  characters where it used to run past 800, so it survives a chat message
  without being wrapped or truncated. Links already in circulation keep working
  until their own expiry — nothing needs to be reissued.

### Added

- Agents can run in parallel across Circles while keeping ordered turns and one
  conversation per agent in each Circle. Device Settings controls concurrency.
- Durable delivery queues recover mentions after reconnects and restarts, with
  delivery status, explicit reply threads, and retry or cancel controls in chat.
- Managed native writes carry per-run change evidence and locks, preserving
  attribution when agents work concurrently. Stopped reply chains remain stopped
  after restarting the daemon.

- `enox update` now updates stable installs itself: it downloads the release
  archive published for your platform, verifies it against the release
  `SHA256SUMS`, installs it, restarts Enoxian, and rolls back to the previous
  binary if the new one fails its health check. Previously a stable install
  could only be updated by rerunning the installer script. `enox update
  --check` reports whether a newer release exists without installing it, and
  `enox update --release <TAG>` installs a specific version.

- Settings has a **Device** tab: rename this device or change your handle
  without starting over, see the handle your agents are addressed by, and see
  which update channel this install follows. Previously the only place to set
  either was the first-run screen, which is unreachable once you have joined a
  Circle.

- Device Settings now has switches for how each agent engages, instead of
  requiring a hand-edit of `agents.toml`: **reads the room** (answer messages
  that name no agent) and **accepts hand-offs** (let another agent pass it
  work), plus the follow-up window for this device. Both per-agent switches are
  off until you turn them on, and turning on "reads the room" says plainly what
  it will send to that agent's provider before it does.

- Agents can be asked to read the room. An agent set `engagement = "ambient"` in
  `agents.toml` is offered every human message and may answer or stay quiet.
  Off by default and per device, because it sends every human line to that
  agent's model provider — the roster marks ambient agents so everyone in the
  Circle can see who is listening. Short messages, agents that just spoke, and
  anything already addressed are skipped without asking a model, and at most one
  ambient reply is offered per message. Files written by an unaddressed turn are
  held for review instead of accepted.
- Agent messages now have a **reply** action. Replying routes to that agent with
  no mention and no timer, which is the only thing that works when two agents are
  mid-conversation with you.
- Replying to an agent no longer needs a mention. For a few minutes after an
  agent answers you, your next message goes back to it — on the same machine
  that ran it — and the composer says so before you press Enter, with Esc to
  leave the conversation. Windows are per person, so two people can hold
  separate conversations with separate agents in one Circle. Configurable with
  `engagement_window_secs` in `agents.toml`; `0` restores mention-only routing.
- Messaging an agent that is already working now queues the message instead of
  failing it into the transcript. Up to four wait per agent, delivered in order
  as separate turns; beyond that the oldest is dropped and said so.
- Agents can now delegate to each other. An agent's reply that mentions another
  agent can wake it, so `@claude` can hand a job to `@codex` without a person
  relaying messages. Off by default and enabled on the receiving side:
  `accept_from = "agents"` under that agent in `agents.toml`. The device that
  spends the tokens decides — there is no setting that lets a remote peer opt
  your agent in.
- A delegation cascade is bounded, so it cannot run away: at most one mention
  per agent reply is honoured, an agent never wakes itself, and the whole
  cascade is capped at `max_relay_turns` agent turns (default 20, hard limit
  50) counted from the human message that started it. Each device enforces its
  own cap, so a peer cannot talk yours into spending more.
- A **stop chain** button halts a running cascade, and anyone in the Circle can
  press it. It stops every further turn on every device rather than
  interrupting the one in flight. A chain that was stopped, or that ran out of
  budget, says so in the activity strip instead of posting to chat.
- Agent replies that were delegated show `via @claude` next to the sender, so a
  reply nobody typed a request for is not mistaken for one that was asked for.
- Proposals record the delegation chain, so a file written by an agent another
  agent asked is distinguishable from one a person asked for directly.
- Chat messages render markdown — lists, tables, headings, quotes, links and
  fenced code blocks, so an agent's structured output is readable instead of
  arriving as raw syntax. Recognised @mentions are still highlighted, except
  inside code, where an `@name` is part of the snippet rather than a ping.

### Changed

- Settings now separate global defaults and Circle preferences through a sidebar scope picker, with clearer typography and a themed, keyboard-accessible dropdown.

- Agents no longer need permission to be reached by other agents. An agent you
  have allowed into a Circle can be handed work by the other agents in it, and
  the per-agent "accepts hand-offs" switch is gone. Your device already decided
  twice — the agent is in your config, and your reaction policy is `push` — and
  a third switch only meant hand-offs failed silently until someone found it.
  How far a chain may run is still yours to set.
- Engagement settings are now global with per-Circle overrides, instead of being
  attached to each agent. Whether an agent reads the room, how long the
  follow-up window lasts, whether mentions run anything, and how far a hand-off
  chain goes can each be set once for everything and overridden in one Circle —
  so the same agent can follow a working Circle closely and stay out of a social
  one. Existing configs are migrated on load; nothing to edit by hand.
- Chat mentions use compact inline labels, agents lead their sender headers, and explicit replies show a linked source preview with a clearer reply composer.

### Fixed

- Imported read-the-room history no longer appears as agents named `~ambient:...`
  or offers retries for old listening observations. Agents that pass now show
  “No reply needed” in their activity history.


- Expanding completed tasks no longer shifts task-list content when the scrollbar appears.

- Settings shows the handle a Circle actually addresses your agents by, rather
  than one assembled from local fields. Your name inside a Circle is fixed when
  you create or join it, so a device whose local handle had since changed was
  shown an address that would silently fail if anyone used it. Settings now
  reads the handle from the Circle, and says that changing your handle applies
  to Circles you join later, not ones you are already in.

- Renaming this device now takes effect immediately instead of at the next
  restart. The name is the middle part of every handle that addresses an agent
  here (`@you/device/agent`) and what this device checks an incoming mention
  against, so a rename used to leave the Circle addressing a name the device no
  longer answered to — with nothing to say why.


- Per-Circle agent settings are easier to find and harder to misread. The
  settings entry said "LOCAL DEVICE" while the panel behind it also held
  per-Circle behaviour, the scope tab named the Circle only in passing, and
  nothing said who can see these settings — they are this device's own answers
  about a Circle, never shared with its members. All three now say so.

- Agent reply status now stays above the chat editor instead of squeezing it beside oversized buttons, including in narrow panels.
- Agents are given the exact handle for every agent in the Circle, instead of a
  display label they had to reconstruct a mention from. The roster read
  `suzy (jessair) [agents: claude]`, leaving an agent to guess that addressing
  it meant `@suzy/jessair/claude` — a guess it could not check. The addressing
  rules are also stated now even on a device that cannot yet place itself in
  the roster, since they do not depend on knowing that.

- An agent can hand work to an agent of the same name on another device. Two
  machines each running a `claude` could not pass anything between them: the
  hand-off was read as the agent mentioning itself and dropped, with nothing
  posted to say why. "Itself" now means the same agent on the same machine.

- A killed or restarted daemon no longer leaves a Circle unable to run any
  agent. A run interrupted mid-flight left a lock behind that nothing expired,
  so every later mention failed with "managed agent '…' is already running in
  this Circle" — naming an agent that was not running. Such a lock is now
  cleared when the daemon starts, since the agent it refers to died with the
  previous one.
- A mention addressed to one device is no longer answered by another. Targeting
  compared the mention against this machine's local identity file, while the
  mention itself is composed from the Circle roster; when the two disagreed —
  after a device rename, or an `~/.enoxian` copied between machines — the
  addressed device ignored the mention and a different one replied in its
  place. Both sides now come from the roster. A device that cannot establish
  what the Circle calls it stays quiet rather than answering for another.
- Agents are now told which device they are running on and how to address a
  specific one. An agent knew its own name but not its machine, so in a Circle
  where two devices run an agent of the same name it could not say which one it
  was, or hand work to a particular sibling — leaving the user to route by hand.
  The brief also now explains that work can be handed to another agent, which
  agents had no way to discover.
- A chat message is no longer lost when sending fails. The composer used to
  clear itself and keep only the staged images, so a long message typed during
  a moment of sync contention was simply gone. The text comes back, and if you
  have since switched circles it waits in that circle's draft.
- A send refused because the Circle was busy syncing now retries by itself
  instead of failing. The daemon asks for a retry in that case and always
  refuses before writing anything, so nothing can be posted twice. Failures
  that are not transient still surface immediately, now with the daemon's own
  explanation rather than a generic "failed to send".
- The service log no longer grows without limit. Routine peer-to-peer churn —
  dial attempts to addresses that were never reachable, and connections closing
  while others to the same peer stay open — was being logged as warnings tens
  of thousands of times a day, which buried the events that do mean something
  and grew the log to gigabytes. Those are now debug-level. A dial that fails
  in the way a mismatched circle key looks still warns, and every failure is
  still listed in `enox status` as before.
- Service logs are rotated when the service starts, keeping three previous
  runs and discarding the oldest, so the log directory stays bounded.
- Rendered markdown shows its list bullets, its list numbering and its links
  again, in both chat and the file preview.
- A half-written chat message, and any image staged with it, now stays with the
  circle it was written in. Switching circles used to carry the unsent message
  across and send it to whichever circle you had moved to.

## [0.8.0] — 2026-09-12

### Changed

- A Circle now keeps its internal state in a single `.enox/` folder instead of
  three siblings (`.enox_crdt`, `.enox_events`, `.enox_proposals`) cluttering
  the top of your working directory. Existing workspaces are moved
  automatically on the next start; nothing is merged or discarded.

### Added

- Storage is now reclaimed instead of growing forever. Decided proposals older
  than 30 days are dropped along with the snapshots and file contents only they
  referenced, and chat images no longer kept by any message are removed too.
  Nothing in either store was ever deleted before, so a long-lived Circle grew
  without bound — one had reached 1.6 GB. A proposal still waiting on a person
  is never collected, however old.

- Build output is no longer synced. Circles now respect `.gitignore` (and
  `.ignore`, and a new `.enoxignore` for enoxian-specific rules), plus a short
  built-in list — `target/`, `node_modules/`, `__pycache__/`, `.venv/`, `venv/` —
  so a project with no ignore file still does not replicate its build
  directory. In one real Circle that was 814 of 1713 tracked files and 1.6 GB of
  stored history. Editing an ignore file takes effect immediately, without a
  restart. Files that become ignored stop syncing but are never deleted, on any
  device.

- Clicking an image in chat now opens it in a viewer inside the app instead of
  a bare browser tab, so you keep your place in the conversation. Arrow keys
  page through every image in the transcript, Escape closes it, and there is a
  download button.
- Attaching an image shows upload progress, so a large file on a slow
  connection no longer looks like nothing is happening.

### Fixed

- Deleting a file or folder now reaches every device, and stays deleted.
  Deletions were only ever sent to devices connected at that exact moment, with
  no record kept — so a device that was offline, or simply mid-reconnect, never
  learned. Worse, that device still believed it had the files and re-created
  them on the device that deleted them, so a deletion could undo itself. A
  device that was away now applies the deletion when it comes back, emptied
  folders are removed rather than left behind, and deleting and re-creating a
  file under the same name works as expected. This holds however the folder was
  removed — deleted outright, or moved to the Trash, which the system reports
  as a single change to the folder rather than one per file.

## [0.7.0] — 2026-09-12

### Added

- Chat supports images. Paste, drag, or pick a PNG, JPEG, GIF, or WebP in the
  web UI and it posts with the message; an image on its own, with no text, is a
  valid message. Attachments replicate to every device in the Circle, arriving
  within seconds rather than waiting for the next reconnect, and a device that
  joins later backfills the images it missed. Images are stored once per Circle
  by content, so posting the same picture twice costs nothing extra.

### Changed

- Building enoxian from source now requires Rust 1.91 or newer, up from 1.88.
  The MLS implementation raised its own minimum, and the protocol stack has to
  move with it. Installing a released binary is unaffected.

- Documented that a Circle with no rendezvous or relay configured falls back to
  a project-operated default server (`relay.enoxian.com`) for peer discovery and
  circuit relay. Behavior is unchanged; it was previously undocumented.

### Fixed

- A mentioned agent now sees what was said in the circle between its turns.
  Previously only its first turn carried the room's conversation, so anything
  members or other agents said while it was away never reached it, and a
  follow-up like "ok go ahead" arrived with no idea what had been decided.
- Mentioning an agent no longer fails with a Unicode encoding error when the
  prompt contains non-ASCII text. Prompts carry em dashes, and anything members
  type in chat, and an agent that read its input in chunks could split such a
  character in half and abort the whole turn.

### Security

- Chat attachments are typed by inspecting their actual bytes, never by the
  name or content type the sender supplied, and only raster image formats are
  accepted — SVG is refused outright, since it can carry script. Attachment
  bytes are served only to authenticated Circle members, and content that does
  not match the hash it arrived under is discarded, so a peer cannot substitute
  a different image for the one a member posted.
- Upgraded the ChaCha20-Poly1305 implementation used for encrypted content
  frames to 0.11. No vulnerability is fixed and the frame format is unchanged;
  the upgrade keeps the cipher on a maintained release line.
- Upgraded the random number generator to `rand` 0.10. Key, token and nonce
  generation continue to use a cryptographically secure generator seeded by the
  operating system. Where a content nonce previously came from an OS entropy
  read that would abort the process if it ever failed, the failure is now
  reported as an ordinary error instead.

## [0.6.2] — 2026-09-11

### Added

- Opt-in daily stable relay updates with checksum verification, peer-identity health checks, and rollback on failed upgrades.

### Fixed

- Keep MLS bootstrap messages intact across periodic updates on slow connections, preventing framing errors that interrupt Circle sync.

## [0.6.1] — 2026-09-06

### Fixed

- Devices no longer stop syncing with each other after about half an hour on
  the relay. A relayed connection is closed by the relay once it hits its
  duration or size cap, and nothing rebuilt it, so two devices on different
  networks drifted apart until a daemon restart. Connections are now rebuilt
  automatically within about thirty seconds.
- The editor no longer gets stuck showing an empty file with the connection
  reading "connecting". When the daemon leaves a sync request unanswered
  because the file is momentarily busy, the web UI now asks again instead of
  waiting forever, so opening a file no longer needs a page reload to work.
- Chat no longer shows "no messages yet" for a conversation that actually has
  messages. If the transcript cannot be loaded on the first try the web UI
  retries, and says so plainly when it still cannot load rather than showing an
  empty room.
- Devices joining a Circle now appear in the roster right away instead of after
  a delay of up to fifteen seconds.

## [0.6.0] — 2026-09-04

### Added

- `enox agent install suzent` is the whole setup for `@suzent`, which is now a
  built-in agent rather than something to wire up by hand. Nothing is downloaded
  and nothing is pinned — enabling it only points the handle at the Suzent CLI
  you already have — and it appears in Device Settings beside the adapters,
  reporting **READY** or asking for the CLI, never for Node.js.
- Plugin manifests in `~/.enoxian/plugins/` accept `kind = "native"` for any CLI
  that speaks ACP itself: `binary` is resolved on `PATH`, `args` carries the
  subcommand, `install_url` says where to get it, and `package`/`version` are
  omitted. Third-party agents no longer need to ship an npm adapter to be a
  first-class plugin.

### Fixed

- The Windows installer works again on Windows 10 machines without .NET
  Framework 4.7.1. It read the machine architecture through an API those
  systems do not have, and strict mode turned the missing property into a hard
  error, so `irm ... | iex` failed with "The property 'OSArchitecture' cannot be
  found on this object" before downloading anything — on hardware the installer
  fully supports.
- `enox member add` and `enox member promote` work again. Both signed a
  different message than the daemon verified, so every attempt was rejected —
  and because the CLI ignored the response status, a rejected request still
  printed `✦ done` while nothing changed. Member operations now report failures
  with the daemon's reason and exit non-zero.
- Typing Chinese, Japanese or Korean in chat no longer sends the message
  mid-word. Enter selects a candidate from the input method's popup, and that
  keypress was being read as "send" — so a sentence went out half-finished on
  the first character anyone typed. Arrow keys, which move through candidates,
  were likewise being intercepted.
- The join animation now names the Circle being joined rather than the name you
  chose for your own device, which made joining "SUZENT-dev" as "suzy" announce
  "joining circle suzy".
- Messages sent just after two devices connect are no longer lost. The initial
  catch-up sends what a peer is missing and the live stream carries everything
  after it, but the two were started in the wrong order, leaving a gap in
  between — anything written in that gap reached the peer by neither route and
  stayed missing until the next reconnect. Chat was the most visible casualty,
  since the first few messages of a session land squarely in that window.
- Approving or removing a member can no longer leave a Circle unable to
  decrypt. Both operations advance the MLS group irreversibly and then publish
  the commit that lets everyone else follow, but the two happened in separate
  steps — so if the control document was momentarily busy in between, the
  group moved on while the commit that described the move was never written,
  and every other device was stranded on the previous epoch. The WebUI then
  invited a retry, which advanced the group again. Each operation is now a
  single unit of work that either completes or changes nothing.
- Circles with many files sync reliably again. Momentary CRDT lock contention
  was treated as permanent failure, so a workspace could connect, complete its
  sync handshake, and still never exchange any changes — the larger the Circle,
  the more reliably it failed.
- Agents no longer appear stale while they are online. A presence heartbeat
  that lost the race for the control document was dropped instead of retried,
  which made a healthy local agent look offline to every peer.
- Peers belonging to a different Circle are no longer dialed, connected to, or
  offered event and proposal data. Every Circle on a device shares one peer
  routing table, so a Circle could spend nearly all of its connection attempts
  reaching other Circles' peers and re-rejecting the same foreign proposals on
  every pass.
- Devices that fell onto different MLS epochs can recover again. The plaintext
  MLS bootstrap exchange is the only way the needed commits can reach a device
  once the encrypted path is already deadlocked, and it was abandoning the
  exchange whenever the control document was momentarily busy — which is its
  normal state. A stranded device stayed stranded, and every sync frame it sent
  failed to decrypt.
- Inviting a device to a Circle no longer leaves a permanent extra "awaiting
  approval" entry. A joining device writes a provisional member entry for
  itself, so the admin's approval arrived as an update to an existing entry
  rather than a new one and was never recognised — leaving a pending request
  that synced back and reappeared on the admin's side after every approval.
- Approving or rejecting a join request updates the WebUI immediately instead of
  lingering until the next refresh. Clearing a pending request emitted no event
  at all, so an open UI kept showing "awaiting approval" for up to fifteen
  seconds after the request was already gone.
- A peer that has been approved no longer keeps showing as "awaiting approval".
  The pending entry is cleared from inside a document observer, which runs
  while the triggering write is still in progress, so the removal lost the race
  essentially every time and the stale entry was never retried.
- Reconnecting to a peer no longer re-sends the whole workspace. Every
  reconnect previously pushed the full history of every document, even when the
  peer already had all of it, and that push ran ahead of live edits on the same
  connection — so in a Circle with many files, chat messages and edits could sit
  behind megabytes of redundant data and never arrive before the connection
  dropped. The catch-up now sends only what the peer is missing, and sends
  nothing at all for documents already in sync.
- Sync and MLS bootstrap failures are logged at warning level instead of debug,
  so a Circle that has silently stopped syncing is now visible in the daemon
  log.

### Security

- Invites are now signed by the member who issues them, and are checked against
  that member's standing at the moment they are redeemed. Removing a member
  therefore invalidates every invite they issued, and an invite admits one
  device rather than any number. Previously an invite was a bearer credential
  that anyone holding the Circle's key could mint, with an expiry nothing
  enforced, and no later change to the Circle could withdraw it. Any member can
  still issue invites — what changed is that the invite now records who did.
  Invites minted by 0.5.0 and earlier carry no grant and are refused, so
  re-issue any that are still outstanding after upgrading.

## [0.5.0] — 2026-08-26

### Added

- Claimed tasks can now be returned to the open pool with `enox unclaim`, so
  another collaborator can claim them.

### Security

- External and Enoxian-managed agents now use short-lived actor tokens bound to
  the issuing device and Circle. Managed runs also bind native file writes to
  their process session, preserving attribution without exposing a token to
  write/edit tools or model context.

### Fixed

- `enox update --dev` no longer fails to build when a pulled revision adds a
  WebUI dependency. The frontend's installed packages are now refreshed
  automatically whenever they are older than the lockfile.

## [0.4.4] — 2026-08-26

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

[Unreleased]: https://github.com/suzent/enoxian/compare/v0.8.0...HEAD
[0.8.0]: https://github.com/suzent/enoxian/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/suzent/enoxian/compare/v0.6.2...v0.7.0
[0.6.2]: https://github.com/suzent/enoxian/compare/v0.6.1...v0.6.2
[0.6.1]: https://github.com/suzent/enoxian/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/suzent/enoxian/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/suzent/enoxian/compare/v0.4.4...v0.5.0
[0.4.4]: https://github.com/suzent/enoxian/compare/v0.4.3...v0.4.4
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
