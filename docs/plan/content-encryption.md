# M17 Content Encryption

M17 adds authenticated message-layer encryption while preserving enoxian's
native-file-IO principle. Local workspace and CRDT stores remain native and
readable by the host; every circle content payload sent to another peer is
encrypted with a key derived from the active MLS epoch.

## Frame Format

All v2 content protocols use this outer frame:

```text
magic[8] | version[1] | purpose[1] | epoch[8] | nonce[12] | ciphertext | tag[16]
```

`magic` is `ENOXC17\0`; integers are big-endian. The header and circle ID are
ChaCha20-Poly1305 associated data. The purpose is CRDT, proposal, or workspace
event. A random 96-bit nonce is generated for every frame.

OpenMLS exports 32 bytes from the current epoch with label
`enoxian-content-v1`. HKDF-SHA256 uses the circle ID as salt and
`enoxian-content-frame-v1/<purpose>` as info. This makes keys distinct across
circles and protocol purposes.

## Protocol Coverage

- `/enoxian/sync/2.0.0` encrypts path and Yjs data together, including control
  state, chat, tasks, awareness, deletions, and session frames.
- `/enoxian/proposals/2.0.0` encrypts the complete JSON message, including
  proposal metadata, manifests, and content-addressed blob bytes.
- `/enoxian/events/2.0.0` encrypts event-log reconciliation, events, attached
  proposal bundles, and blobs.

Length/count fields needed to delimit transport frames remain outside the
ciphertext and reveal traffic shape, not semantic content.

## Bootstrap And Epoch Changes

Encryption creates a necessary bootstrap exception: a new device cannot decrypt
the current content epoch until it receives an MLS Welcome, and an offline
device cannot decrypt a newer epoch until it receives missed commits.

The persistent `/enoxian/mls-bootstrap/1.0.0` stream carries only membership
delivery material. It runs inside the circle-PSK and Noise transport and sends
KeyPackages, signed membership records, targeted Welcomes, removal tombstones,
and the append-only MLS commit sequence. Content protocols wait briefly for the
requested epoch while bootstrap applies these artifacts.

Each daemon retains the eight most recent exporter secrets in memory for frames
already in flight. Secrets are not persisted. A removed member can apply its
Remove commit but OpenMLS will not export the new epoch secret, providing future
content secrecy after removal. An offline retained member replays commits and
derives the same current exporter secret on reconnect.

## Security Boundary

The MLS bootstrap is membership metadata, not circle content. Possession of the
stable circle PSK may expose that bootstrap metadata and traffic shape, but it
does not yield current content after MLS removal. Local disk encryption remains
the responsibility of the host; see
[the security model](../concepts/security.md#data-at-rest).
