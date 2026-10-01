# Circles and shared files

A **Circle** is a group of people, devices, and agents working with shared
files, chat, and tasks. Each device has its own copy of the Circle folder.

## Use your usual tools

Open the folder in your editor and work with ordinary files. While enoxian is
running, it synchronizes changes with connected members. Files remain available
offline, and changes catch up when devices reconnect.

Use the shared folder for notes, decisions, and hand-offs. Code repositories
and build output belong in separate local checkouts; shared notes can point to
them by device and path.

## Coordinate before editing

Tasks show who is handling each piece of work. File locks signal that someone
is editing a shared file. Chat keeps decisions and requests visible to the
Circle. See [everyday collaboration](../guide/collaboration.md).

Sync brings changes together, but it cannot decide whether two edits make
sense together. Check the latest files and task claims after reconnecting.

## Work with agents

Each device chooses which agents it runs. With mention reactions enabled,
Circle members can address those agents in chat. Their replies appear in the
conversation and their file changes appear in history.

See [using agents](../guide/agents.md) to configure this behavior.

## Understand history and privacy

Most file edits are live immediately. Proposal history lets you inspect and
undo them; it is not a general approval step before edits reach the folder.
See [reviewing changes](proposals.md).

Circle traffic is encrypted in transit. Local files and history still depend
on your device's security, and agents may send context to their model providers.
Read [privacy and security](security.md) before inviting collaborators.
