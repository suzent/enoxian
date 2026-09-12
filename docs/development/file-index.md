# File index

How a Circle should decide what files exist, so that deletion, binary content,
and renames stop being special cases. Forward-looking: none of this is built.

Background reading, not restated here:
[reference/p2p-protocols.md](../reference/p2p-protocols.md) for the sync
protocols and frame formats, and [concepts/proposals.md](../concepts/proposals.md)
for how file changes become reviewable.

## The problem

A Circle models a file as a Yjs `Text` document keyed by its relative path. That
is the right model for *content being co-edited* and the wrong model for *a file
existing*. Yjs has no notion of existence, deletion, or bytes, so every question
about a file's lifecycle has had to be answered somewhere else — or not at all.

Three shipped bugs come from this single mismatch:

**Binary files are silently dropped.** The watcher reads with `read_to_string`
and skips anything that is not valid UTF-8. A PNG placed in a Circle folder
never enters `state.docs`, so it is absent from the handshake doc set and from
`list_files` too. From every peer's point of view it does not exist, and nothing
warns anyone. `yrs::Text` accepts `&str`, so this is structural, not an
oversight.

**Deletion had no representation.** A deleted file simply stopped being in
`state.docs`. That worked only while every peer was connected at that instant.
Fixed for now by a tombstone map in the control doc, but note what that fix is:
a second, parallel place where file lifecycle is tracked, because the first
place cannot express it.

**Deletion used to undo itself.** `all_doc_paths` advertises whatever a peer
currently holds, so a peer that missed a deletion re-offered the file and
re-created it on the device that deleted it. The root cause is that *absence of
a document is ambiguous*: it means both "deleted" and "not synced yet". No
amount of care in the sync loop fixes an ambiguous data model.

Renames are the next one in the queue. Today a rename is a delete plus a create,
so the file's history does not follow it and a large file crosses the wire
twice.

## Invariants

These hold today and must survive any change here.

- Live co-editing keeps character-level merge. Two people typing in the same
  file must not produce a last-writer-wins clobber.
- File IO stays native. The daemon coordinates; it is not a file proxy
  (`AGENTS.md`).
- Content is encrypted with MLS-derived per-epoch keys and never leaves that
  boundary.
- A removed peer cannot read or write Circle content.
- The control doc stays small. It is fully replicated to every device and held
  in memory, which is why proposal history was deliberately kept out of it.

## The design

Split the two jobs that Yjs is currently doing.

**A file index owns lifecycle.** One entry per path, in a CRDT map:

```
path -> {
    hash:     blake3 of the content, or null if deleted
    size:     bytes
    mtime:    unix ms, from the writing device
    version:  per-device counter, for ordering without trusting clocks
    deleted:  bool
    author:   peer_id of the last writer
}
```

Existence becomes an explicit, replicated fact rather than an inference from
presence in a map. Deletion is `deleted: true`, which is unambiguous on arrival
and survives a disconnect. Binary is free, because the index never reads the
bytes — it holds a hash. A rename is one entry gaining a path and another
tombstoning, with the same hash, so content never crosses the wire twice.

**Content moves as blobs.** This already exists and works:
`BlobStore` is content-addressed, and the `\0blob-want/` / `\0blob-data/`
exchange on the live sync stream fetches blobs on demand over MLS-sealed frames,
with hash verification on receipt. It carries chat attachments today. Nothing
new is required to carry file content the same way.

**Yjs keeps live co-editing only** — documents someone actually has open. A
session starts from the blob named by the index, edits merge character-by-
character as they do now, and on quiesce the result is written back as a new
blob and a new index entry. Yjs stops being the system of record for files and
becomes what it is good at: a live editing session.

This is deliberately the same shape the proposal subsystem already arrived at
independently: `adapters.rs` falls back to a hash-only `binary::diff` for
non-UTF-8, because content addressing is how you handle a file you cannot merge.

### Where the index lives

Not in the control doc. 1713 entries for one folder is ordinary, and the control
doc is fully replicated and in memory. It should be its own CRDT document,
synced over the existing stream like any other doc, so it can be large without
making presence heartbeats expensive.

## Why not an off-the-shelf library

Worth stating plainly, because "use a library" is the right instinct and the
answer is still no.

There is no lightweight, embeddable, peer-to-peer file-sync crate that plugs
into an existing transport. What exists:

- [`filesync`](https://crates.io/crates/filesync) — syncing to arbitrary
  sources such as S3 buckets. Not peer-to-peer.
- [`syncthing-rs`](https://github.com/JayceFayne/syncthing-rs) — a REST client
  for *controlling* a Syncthing daemon. It does not implement BEP; it assumes
  you shipped Syncthing alongside.
- [Ensync](https://lib.rs/crates/ensync) — an encrypted synchronizer, but an
  application built around a central server location.
- [`optra`](https://github.com/dyule/optra) — a remote file sync engine,
  long unmaintained.
- [`iroh-docs`](https://github.com/n0-computer/iroh-docs) +
  [`iroh-blobs`](https://docs.rs/iroh-blobs) — genuinely close. Multi-dimensional
  key-value documents with an efficient range-based reconciliation protocol,
  over BLAKE3-verified content-addressed blobs. This is, more or less, the
  design above, already written and maintained.

The blocker for iroh is the transport. It is a QUIC stack with dial-by-public-key
and its own NAT traversal, and `iroh-docs` is a meta-protocol over `iroh-blobs`
and `iroh-gossip`. A Circle's security model is MLS-derived per-epoch content
keys over libp2p, with removed-peer tombstones rechecked between protocol
phases. Adopting iroh means either running two independent networking stacks
with two trust models — and content leaving the MLS boundary, which is not
acceptable — or reimplementing its protocols on our transport, at which point
the library is not being used.

So: take the design, not the dependency. The parts worth stealing outright are
BLAKE3 verified streaming (we use SHA-256; BLAKE3's tree hashing allows verified
*range* requests, which matters for large files) and range-based set
reconciliation instead of advertising every path on every handshake.

## Staging

Each step is useful alone and none requires the next.

1. **Index alongside the existing docs.** Write entries on every watcher event;
   change no sync behavior. Purely additive, and it makes the index's accuracy
   observable before anything depends on it.
2. **Handshake reconciles the index** instead of `all_doc_paths`. Deletion and
   resurrection stop being special cases; the tombstone map added in #129 is
   subsumed and can be deleted.
3. **Binary files sync**, by routing non-UTF-8 content through blobs. This is
   the first user-visible win and the one most likely to be asked for.
4. **Yjs narrows to open documents.** The largest change, and the one to do last
   — it touches the editor path, conflict copies, and the proposal engine's
   interactive-write fold.
5. **Renames**, once entries carry hashes: a rename is an index operation.

## Open questions

- **Rename detection.** Same hash appearing at a new path within a window is the
  usual heuristic, and it is only a heuristic. A copy looks identical to a
  rename until the original is deleted. Is being wrong occasionally acceptable
  if the content transfer is avoided either way?
- **When does an editing session end?** Writing back a blob on every keystroke
  is wasteful; writing on close loses work on a crash. A quiesce timer is the
  obvious answer and needs a number attached to it.
- **What happens to CRDT history on write-back?** Today a file's Yjs history is
  its merge history. If a file becomes a sequence of blobs, concurrent offline
  edits by two devices become a genuine conflict needing a conflict copy, where
  today they merge silently. That may be more honest — silent merges of
  non-adjacent edits are not always wanted — but it is a behavior change.
- **Index retention.** Tombstones need a retention window for the same reason
  the current ones do (90 days). Does an index entry for a live file ever need
  pruning, or only deleted ones?
- **Migration.** An existing Circle has Yjs docs and no index. Building the
  index from disk on first start is easy on one device and racy across several
  starting at once. Does the owner build it, or does every device build and
  merge?
