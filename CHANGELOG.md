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

- `enox status` lists who currently holds each bound path, and `/status`
  returns them as `locks`, so you can check a lock before editing instead of
  finding it through a failed `enox bind`.
- `enox claim --takeover` takes a task from a holder that went away, so a
  claim cannot keep a task stuck forever. The previous holder is recorded and
  shown by `enox tasks` and in the `task_claimed` event.
- `enox bind` locks are now leases: 10 minutes by default, `--ttl` up to an
  hour, renewed by binding again. A lock nobody renews frees itself, so an
  agent that crashed no longer holds a file forever. `enox bind --takeover`
  takes a lock from a holder that has gone away and records who held it, and
  `enox status` shows when each lock ends.
- Editing a file someone else holds from another device raises a
  `lock_violated` event on both devices, so the holder learns about it.

### Changed

- Agents are told which machine each other agent is on. Two agents with the
  same name used to look identical in the conversation an agent is shown —
  "claude: …" twice, one of them possibly the reader itself — and in who else
  was offered a message, who is already working on one, and who delegated a
  request. They now read "claude (on suzy/jessair)", and `enox inbox` shows
  claims the same way.
- `enox bind` no longer makes the file read-only. It could not stop anyone
  who owned the file, only applied on the device that bound it, and blocked
  the holder's own tools from writing. Locks are now advisory throughout, and
  releasing a file bound by an earlier version makes it writable again.

### Fixed

- An agent's answer is no longer lost when the Circle happens to be busy the
  moment it finishes. Posting the reply gave up at once if anything else was
  writing Circle state — a routine save, an incoming sync — so a turn that had
  already done all its work failed and its answer was discarded over a delay of
  milliseconds. It now waits for its turn, for up to ten seconds.
- When agents with the same name fail on two different machines, both failures
  are now reported. The notice began "claude could not answer" either way, so
  whichever came second looked like a repeat of the first and was suppressed.
  Notices now say which machine: "claude (on suzy/jessair) could not answer".
- `enox claim` on a task another agent or device already holds now fails
  with the current claimant's name, instead of silently taking the task over
  while both sides believe they own it. Re-claiming your own task still
  succeeds.
- `enox done` now only works for the task's claimant, and a done task can no
  longer be claimed. Before, anyone could mark someone else's task done and
  then claim it, getting around the claim check.

- A self-hosted relay's automatic updater now updates itself. It ships with
  each release and is verified against the release's checksums, so a change
  it could not have anticipated no longer strands it. An updater installed
  before 0.9.0 cannot do this and fails every daily run on the new
  `enox --version` format; replace it once by hand as described in
  `scripts/rendezvous/README.md`.

## [0.9.2] — 2026-09-24

### Added

- `enox inbox` shows what is waiting for an agent: messages this device has
  queued for it, and the room's unanswered questions along with who is already
  working on each. `enox inbox claim <id>` takes a message so agents reading the
  room — on every device — leave it to you; the claim expires by itself, and
  `enox inbox release` gives it back sooner. Agents are told the inbox exists,
  so one handed a single message can see what else is waiting.

### Changed

- An agent reading the room no longer starts on a message another agent is
  already working on. It used to spend a full turn and only then discover,
  through Held Draft, that its answer was redundant; it now backs off before
  starting. Agents offered the same message together on one device are not
  affected, so asking for two responders still gets two.

### Security

- On Linux, finishing an agent run no longer kills every process you own.
  Cleaning up after an agent signalled its process group with the system
  `kill`, and procps-ng 4.0.4 (Ubuntu 24.04) reads a group that has already
  exited as "every process" — so a completed turn could take down your
  terminal, editor and desktop session along with the daemon. macOS was not
  affected.

## [0.9.1] — 2026-09-23

### Fixed

- Agents in a busy Circle no longer stop responding, with the activity list
  filling up with "ambient observation expired on restart" when nothing
  restarted. Saving the execution queue briefly marked it as unavailable, and
  anything checking at that moment concluded the queue was dead and tore the
  agent loop down — which expired the waiting turns, re-queued them, and
  triggered the same collapse seconds later. The busier the Circle, the more
  often it happened.

- A restart no longer quietly halves how many agents read a message. When a
  Circle offers a message to two agents and the daemon restarts before they
  run, both are now put back — previously only one was, so the room went from
  two responders to one without saying so.
- Agent activity no longer asks you to retry work the device already retried
  by itself. A turn killed by a restart and automatically picked up again was
  listed under "Needs attention" with a "Try again" button, so a successful
  recovery looked like an outstanding failure. Those are shown as history now,
  and only a failure nothing has picked up asks for attention.

- A restart while agents were queued no longer makes the room report that
  nobody could answer. Turns killed before they started were counted as failed
  attempts, so with two agents reading a room a single restart used up the
  whole retry budget and the message was written off — having never run at all.
  A turn that never started is now put back, and only runs that actually
  executed count against the limit.

### Added

- An agent reading the room is now asked what to do when someone answers first.
  A turn takes seconds to minutes, and the room does not wait — so if another
  agent replies to the same message while this one is writing, its draft is held
  and handed back rather than posted on top or discarded: the agent sees what
  was said and chooses to drop it, post it unchanged, or replace it with
  something that accounts for the answer. Asked once per turn, and never for a
  turn you requested by name.

- An agent reading the room is now told that is what it is doing. Its prompt
  previously opened by asserting it had been @mentioned and framing the message
  as a request to answer, then appended a paragraph at the end saying it had not
  been addressed after all. An unaddressed turn now reads as overheard from the
  start, is told the room is a group conversation mostly not aimed at it, and is
  told which other agents are weighing the same message — so declining is a
  normal outcome rather than one the prompt has just argued against.
- An agent now sees images and files attached to the message that woke it, with
  their names, types and a URL to fetch them. A wordless screenshot previously
  started a turn whose entire instruction was empty.

- Agent activity now explains why nothing ran. A "Not picked up" section lists
  the messages this device saw and declined, with the reason — too short to be
  worth a turn, every listener had just spoken, the agent isn't configured here,
  this device is set to pull. Standing configuration problems are called out at
  the top, including agents listed as reading the room that aren't configured
  and so are silently ignored.
- A turn that fails now says so in the room. Previously a run that crashed or
  timed out only flashed an indicator for 45 seconds and left nothing in the
  transcript, so an unanswered message was indistinguishable from one nobody
  cared about. One system line is posted per agent, throttled, and suppressed
  when another agent has already answered.

### Fixed

- A message read by an agent that then crashes or times out now passes to
  another agent instead of going unanswered. Previously the first attempt was
  the only one: whichever agent was picked consumed the room's single chance to
  reply, and a broken adapter meant silence. Tries are capped per Circle, and
  when they run out the room is told so once, rather than being left looking
  ignored.
- A turn queued when the daemon restarted is now picked up again rather than
  discarded.
- An @mention from a device whose clock is behind yours is no longer ignored.
  Whether a message was old enough to skip was decided by comparing the
  sender's clock against the moment this device started listening, so a peer
  running slow could have every mention it ever sent to this device dropped,
  without a trace and for as long as the Circle existed. What was already in
  the room when listening started is now remembered directly instead.
- An agent reading the room is no longer silent because of a clock. Whether an
  unaddressed message got a turn was decided by comparing the sender's clock
  against the receiving device's, so a peer running a minute slow could never
  reach a listener — not after a delay, but never, because its messages arrived
  already too old to consider. Messages that arrived late for any other reason,
  such as a peer syncing after a disconnection, were dropped the same way and
  without a trace. A message is now read the first time a device decides about
  it, however long it took to get there.
- Coming back to a room that moved on no longer means missing all of it. After
  a restart, a disconnection, or a peer syncing a backlog, the most recent
  messages still get a turn — one by default, configurable per Circle — and the
  rest are marked as read without one, so catching up costs the same as keeping
  up.
- Several messages sent in quick succession are now considered together, so a
  thought typed across three lines gets one reply informed by all of them
  instead of dying on the length limit twice and being answered once out of
  context.
- An agent named in the read-the-room list with different capitalisation than
  its configuration is no longer silently ignored.
- Quoting a person to ask the room a question now reaches an agent. Using Reply
  on anyone's message previously meant no agent would ever see it, because the
  message counted as aimed at someone but resolved to nobody. Replying to an
  agent still goes to that agent alone.
- An agent whose configured name contains a character that cannot appear in a
  mention is now ignored with a warning instead of loading and misbehaving.
- With a varying number of listeners, how many a given message gets no longer
  changes depending on when the daemon last restarted.

### Changed

- An agent reading the room no longer runs if another agent answered the message
  while it was queued behind it.
- An agent reading the room is given three minutes to respond rather than the
  half hour an addressed request gets, so an unprompted aside cannot hold up
  work someone actually asked for. Configurable per device.

- Circle identities now use four dithered visual families assigned from Circle
  IDs. Working marks animate, unread messages use a halo, and hovering a mark
  enlarges it with pointer movement. A skippable, reduced-motion-aware identity
  assembly replaces the old 3D Circle entry effects.
- The conversation shares a quieter workspace with contextual Activity, Members,
  Tasks, and Workspace panels. Circle settings live in Members; named actions,
  compact invitations, and clearer empty states replace ambiguous sidebar controls.
  Sidebar edges remain resizable without visible drag handles. Agent messages
  now use deterministic geometric avatars.

- Sidebar controls now blend into panel headers without a separate app header.
  Larger labels and clearer text improve readability, and conversations keep
  a centered reading width when sidebars are collapsed.

### Added

- Three more ACP agents can be installed from the built-in catalog: `pi`
  (through the pinned `pi-acp` adapter, driving the `pi` CLI you installed),
  `hermes`, and `openclaw` (both speak ACP from their own CLI, so nothing is
  downloaded or pinned). `enox agent plugins` lists them and Device Settings
  offers them once their CLI is on `PATH`.
- `enox --version` now names the build channel and the commit it was built from
  (`enox 0.9.0 (dev, 1a2b3c4d5e6f)`), so a binary built with `enox update --dev`
  is distinguishable from a published release in a bug report.

### Fixed

- An agent's prompt no longer repeats chat lines it has already been shown:
  catch-up context that ran ahead of the agent's cursor is remembered between
  turns, a thread ancestor already quoted in the room context is not quoted
  again, and a resumed conversation is no longer fed the agent's own earlier
  posts, which it already remembers.
- Agents that read the room now answer questions written in Chinese, Japanese,
  and Korean. The "is this worth a turn" floor counted characters, so a CJK
  message was judged too short to be worth answering however much it said, and
  only explicit mentions worked in those Circles.
- The lock log no longer grows without bound. Agents cycling file locks could
  add hundreds of thousands of entries, which slowed every later lock, stalled
  saves, and could grow a Circle's stored state past the point where it would
  load at all. Settled entries are now discarded; existing oversized Circles are
  repaired on next start.
- A Circle that is slow to load no longer prevents other Circles from starting.
  Circles now start independently, so one stuck Circle cannot strand the rest or
  stop newly added Circles from being picked up.
- `enox enable` and `enox disable` now report whether the running daemon
  actually started or stopped the Circle, instead of always reporting success.

## [0.9.0] — 2026-09-15

### Added

- Agents can delegate work to other agents in a Circle, with attributed replies,
  a configurable turn budget, and a **stop chain** action available to everyone.
  Stopped chains remain stopped after daemon restarts.
- Agents can read the room and answer unaddressed human messages. This is off by
  default because it sends those messages to the agent's model provider. Choose
  how many agents listen per message, with fair rotation between agents; files
  written by an unaddressed turn are held for review.
- Agents can run in parallel across Circles, with configurable concurrency,
  ordered turns, and one conversation per agent in each Circle. Durable delivery
  queues recover mentions after reconnects and restarts, and chat exposes
  delivery status and retry or cancel controls.
- Chat renders Markdown, including tables, lists, links, and fenced code blocks.
  Explicit replies link to their source message and route back to the agent
  without requiring a mention.
- Device Settings lets you rename the device, change your handle, view the update
  channel, and configure agent engagement. A scope picker separates global
  defaults from this device's per-Circle overrides.
- `enox link` pairs another device using a four-word code and a matching six-digit
  confirmation number, carrying over joined Circles. Any linked device can link
  another device without transferring the user root key or recovery phrase.
- The CLI and web interface create short invites backed by sealed contents on
  the relay, with automatic fallback to self-contained invites. Use
  `enox invite --long` for a self-contained link. Existing invite links keep
  working until they expire, and joining a short invite starts the Circle.
- Relay operators and clients can query `GET /version` for the running version
  and support for short invites and device linking.
- `enox update` supports stable releases, verifies archive checksums, restarts
  Enoxian, and rolls back if the new binary fails its health check. Use
  `--check` to check for updates or `--release <TAG>` to select a release.

### Changed

- Agent engagement settings now use global defaults with per-Circle overrides,
  with existing configurations migrated automatically. Agent hand-offs no
  longer require a separate per-agent opt-in; configured agents in Circles with
  the device's `push` reaction policy can receive them.
- Follow-ups use explicit reply threads by default. An omitted
  `engagement_window_secs` now defaults to `0`; set it to `180` to enable
  three-minute follow-up routing. Explicitly configured windows are preserved.
- Chat mentions use compact inline labels. Agent activity shows device names,
  source messages, and plain-language outcomes; completed runs are collapsed
  and legacy listening observations are hidden.

### Fixed

- Agent targeting now uses the Circle roster, preventing replies from the wrong
  device after a rename or copied identity. Agents receive exact handles and
  their own device context, and can hand work to an agent with the same name on
  another device.
- Daemon restarts clear abandoned managed-agent locks and wait out briefly
  inherited execution inbox locks, avoiding stuck agents and startup failures.
- Device renames take effect immediately and no longer invalidate attestations.
  Settings shows the handle the Circle actually uses and explains that handle
  changes apply to Circles joined later.
- Failed chat sends restore the draft, transient sync contention retries
  automatically, and unsent text and images stay with their original Circle.
- Long agent activity text truncates without overlapping nearby statuses, reply
  status stays above the editor in narrow panels, and expanding completed tasks
  no longer shifts the task list when a scrollbar appears.
- Markdown list markers and links display correctly in chat and file previews.
- Automatic-admission Circles retry pending requests after restarts and delayed
  key packages, clear stale requests for existing encrypted-group members, and
  show admission errors in the member panel.
- Discovery and reconnect attempts share per-peer backoff, including across
  short-lived connections, reducing relay request bursts during repeated
  disconnects.
- Routine peer connection churn no longer floods service logs with warnings.
  Logs rotate at service startup, retaining three previous runs.

### Security

- Devices prove their user identity through verifiable attestation chains.
  `enox member list` marks unverified owners, and `enox identity show` reports
  attestation validity and chain depth. Older peers remain supported as
  unverified members.
- `enox member distrust` blocks a user identity within a Circle, including future
  devices proving that identity. `enox member trust` reverses the decision, and
  the member list marks distrusted identities.
- Recovery phrases are shown once at identity creation and no longer written to
  disk. `enox identity show` flags phrases retained by older installs;
  `enox identity forget-phrase` removes them after showing them one last time.
- Files containing key material (`identity.toml`, Circle `config.toml`, and
  `admin.key`) are written with owner-only permissions (`0600`). Existing files
  have their permissions tightened when read.
- `enox link` and rendezvous address resolution no longer send the local daemon's
  API token to remote bootstrap servers.
- Managed native writes carry per-run change evidence and locks, preserving
  attribution when agents work concurrently.

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

[Unreleased]: https://github.com/suzent/enoxian/compare/v0.9.2...HEAD
[0.9.2]: https://github.com/suzent/enoxian/compare/v0.9.1...v0.9.2
[0.9.1]: https://github.com/suzent/enoxian/compare/v0.9.0...v0.9.1
[0.9.0]: https://github.com/suzent/enoxian/compare/v0.8.0...v0.9.0
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
