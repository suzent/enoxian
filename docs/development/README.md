# Design specs

Forward-looking plans: proposed designs, open questions, and work not yet built.

**Nothing here describes shipped behavior.** The rest of `docs/` documents the
system as it actually is; this folder is the one place that may describe things
that do not exist. When a spec ships, fold what users need into the relevant
guide or concept page and delete the spec — Git history keeps it. A spec that
has drifted from the code is worse than no spec, so retire them promptly.

Currently open:

- [read-the-room.md](read-the-room.md) — what an agent reading a room still
  cannot do, and the questions that want measurement before design.
- [file-index.md](file-index.md) — deciding what files exist, so deletion,
  binary content, and renames stop being special cases.
