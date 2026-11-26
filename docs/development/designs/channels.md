# Channels

How a Circle splits its one chat room into several, and how an agent's
conversation follows that split.

Background reading, not restated here:
[read-the-room.md](read-the-room.md) for how agents decide to speak, and
[architecture/internals.md](../architecture/internals.md) for the agent runtime.

## Scope

**In scope: topic channels.** Every member of the Circle can read and post in
every channel. A channel is a partition of the conversation, not an access
boundary.

**Out of scope: private channels** (visible to a subset of members). The control
document replicates to every member and is encrypted under the Circle's single
MLS group key, so nothing in it can be hidden from a member. A private channel
needs its own MLS subgroup and its own document — effectively a second Circle —
and deserves its own design.

**Channels are not threads.** A thread is a chain of replies *inside* a channel
(`reply_to` / `thread_root`). The two stay separate concepts; nothing here keys
on `thread_root`, which read-the-room §5 found unused in practice.

## Storage

| Key | Contents |
|-----|----------|
| `chat` (`CHAT_KEY`) | `#general`. Unchanged, so existing transcripts need no migration. |
| `channel_chat` (`CHANNEL_CHAT_KEY`) | Every other channel's messages, each tagged `channel: <id>`. |
| `channels` (`CHANNELS_KEY`) | `channel-id → JSON { name, purpose, created_by, created_at, archived }` |

In code a channel is `Option<&str>`: `None` is `#general`, `Some(id)` any other.

Why `#general` stays in its own array rather than gaining a field:

- **Old peers.** `ChatMessage` deserializes leniently, so a peer predating
  channels would read a channel-tagged message in `chat` as a `#general`
  message, and its agents would answer it there. `channel_chat` is simply
  invisible to it.

The other channels share one array rather than one each: Yrs root keys cannot
be enumerated cheaply on load, retention and save/restore handle one more array
instead of an open-ended set, and positions within a channel are derived by
filtering on `channel`. Retention (`store/control.rs`) applies per array.

`AppState` reads:

- `transcript()` — `#general` only, as before.
- `channel_transcript(channel)` — one channel, in order.
- `all_messages()` — every channel, `#general` first, **not** sorted by time.
  For lookups by id (`find_message`, the inbox, execution, warden episodes);
  sort it first where order matters.

Channel ids are UUIDs; names are display labels: lowercase `a-z 0-9 - _`, up to
32 characters, `#` stripped. Creating refuses a name a live channel this peer
knows already has. Two peers apart can still both create `#ops`; both channels
stay, and name lookup (`channel::resolve`) prefers a live channel over an
archived one and otherwise reports the ambiguity rather than guessing. Merging
two transcripts after the fact has no right answer. Channels are archived,
never deleted; their history stays readable, and an archived channel accepts
only replies to its existing messages.

## Agent sessions: one per channel

An agent has one ACP session per **(circle, agent, channel)** — a *lane*,
`(agent, Option<channel>)` in code.

```text
agent_sessions/<agent>.session             # #general (the pre-channel path)
agent_sessions/<agent>/<channel-id>.session  # every other channel
```

The record (`session_id`, `last_seen_message`, `delivered`) keeps its shape; only
its key grew, and the `#general` path is the legacy one, so existing
conversations carry on.

The run lease, liveness, worker, execution inbox and ambient preemption all key
on the lane. Turns of one agent in different channels run in parallel, bounded
by the device-wide `max_concurrent_runs`; within one channel they stay serial.

### Why not one shared session with per-channel cursors

That keeps the agent's full working memory across channels, which is real
value: it edits one shared folder from every channel. It was rejected for:

- **Concurrency.** One ACP session cannot serve two turns at once — loading the
  same session twice concurrently is undefined in ACP. Channels make
  simultaneous requests normal, so a shared session either serializes all of
  them or needs temporary side sessions whose replies must later be fed back,
  breaking the "a resumed session already holds its own posts" rule in
  `context.rs`.
- **Bleed.** The previous turn in the session may come from another channel,
  and the model tends to continue it.
- **Blast radius.** A failed `session/load` loses every channel's memory at
  once, and every other channel's cursor would then claim delivery the new
  session does not hold.

What a per-channel session loses is recovered, partly, by the cross-channel
index below; conclusions that matter beyond one channel belong in a note in the
shared folder.

Two parallel turns of one agent can touch the same files. Each is its own change
session with its own base snapshot, so the proposal engine detects conflicts;
the brief tells agents to `enox bind` shared files.

## Where replies go

The daemon decides, never the model:

- A run's own reply posts to the channel of its trigger.
- `ENOXIAN_CHANNEL` is set on every agent run (ACP and argv drivers), so
  `enox say` and `enox chat` default to the run's channel.
- A post from an agent run that names no channel defaults server-side to the
  run's channel; a reply (`reply_to`) inherits its parent's channel.

## Prompt context

`build_delivery` works as before, over the channel's own lines. It then appends
`channel_index` (`agent/context.rs`):

```text
You are in #bugfix — crash triage. Your reply posts here.
Other channels in this Circle — separate conversations you are not shown here.
Read one with `enox chat --channel <name>`; post into one with
`enox say --channel <name> "…"`. Only do so when it bears on your work:
  #design — UI work (7 new, 1 mention you)
  #general (3 new)
  #ops (nothing new)
```

- "New" counts lines after this agent's cursor for that channel; mentions are
  counted from the structured `mentions` field.
- Ranked by mentions, then by whether anything is new, then by recency; at most
  12 listed, the rest summarized with a pointer to `enox channels`.
- Archived channels are left out unless the agent is in one.
- Empty for a Circle that has only `#general`, so prompts there read exactly as
  they did before channels.

The index is pushed rather than left to the CLI because an agent does not go
looking in a channel it has no reason to think about.

## Engagement, follow-ups, ambient turns

- The **engagement window** is resolved from the channel's own lines. A
  follow-up in `#ops` does not route to the agent engaged in `#design`. The
  API takes `?channel=` on `/chat/engagement` and `/chat/engagement/exit`.
- **Called after a pass** (`agent/outcome.rs`) only counts a later call in the
  same channel.
- **Relay budgets and chat activity** key on message ids and needed no change.
- **Unaddressed turns** use the lane, so an ambient turn in one channel neither
  blocks nor is preempted by work in another. Skipping one loses nothing: the
  next turn in that channel delivers it as context.

## API

| Route | |
|-------|-|
| `GET /circles/{id}/api/channels` | Every channel, `#general` (id `null`) first, with `message_count`, `last_message_id`, `last_ts`. |
| `POST /circles/{id}/api/channels` | `{ name, purpose?, agent_id }`; 400 on a bad or taken name. |
| `PATCH /circles/{id}/api/channels/{ref}` | `{ purpose?, archived? }`; `#general` cannot be changed. |
| `GET /chat?channel=` | One channel's transcript (id or name). |
| `POST /chat` | Takes `channel` in the body. |

The chat stream (`/chat/stream`) carries each message's `channel` on
`message_posted`, and sends `channels_changed` when a channel is created or
changed. The inbox's open list carries each message's channel name.

## CLI

```bash
enox channels                              # list: name, purpose, message count
enox channels create <name> [--purpose …]
enox channels archive <channel>
enox channels unarchive <channel>
enox chat --channel <name> [--since <ts>] [--follow]
enox say --channel <name> "…"
```

`--channel` takes an id or a unique name, and defaults to `ENOXIAN_CHANNEL`.
`enox inbox` prefixes messages outside `#general` with their `#channel`.

Reading a channel through the CLI never advances that channel's cursor. The
cursor records what the daemon put in front of the agent, nothing else.

## Frontend

Channels are listed in the left sidebar, nested under the active circle's row
(`ChannelList`): one row per live channel (`#general` first), an unread count
on rows other than the open one, an Archive action on hover (on touch, on the
open channel only), and an inline form to create a channel. Other circles stay
collapsed; the chat stream only covers the active circle, so their channels
would have no unread counts to show. In the phone drawer, picking a channel
closes the drawer.

The chat header shows the open channel and its purpose after the circle's
name (`DEMO / #design UI work`), so the channel is visible with the sidebar
collapsed; the composer's placeholder names it too.

`ChannelContext` holds the channel list, the open channel (remembered per
circle for the session) and unread counts, shared by the sidebar and
`ChatPanel`. `ChatPanel` loads and streams only the open channel, posts into
it, and reads the engagement window for it. Messages for other channels
arriving on the stream bump that channel's unread count instead of rendering.
A `channels_changed` event refreshes the list.

## Not built

- **Subscriptions.** An `agents.toml` option such as `channels = ["*"]` to limit
  which channels offer an agent unaddressed messages. Today ambient agents read
  every channel; a mention wakes an agent in any channel.
- **What the agent did elsewhere.** The index reports how much is new in each
  channel, not what this agent itself posted or changed there (its own messages,
  managed runs, proposals by trigger id).
- **Unread across sessions.** The frontend's unread counts live in memory and
  start from zero on reload; there is no per-viewer read cursor.
- **Renaming** a channel. Only purpose and archived can change.

## Open questions

- Whether an agent may start a turn in a channel it is not subscribed to when a
  person there replies to one of its messages (`reply_to`) — probably yes, as
  an addressed turn, once subscriptions exist.
- Whether a per-agent option to share one session across channels is worth
  offering later, for agents that are only ever mentioned one at a time.
