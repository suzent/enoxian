# Invite format and short-link storage

For creating and sharing invites, see the [user guide](../../guide/invite.md).

## Wire versions

`enox` mints **v2** links for libp2p Circles and **v3** links for Circles on the Iroh transport (in development), and decodes all three. A `v1` link that is already in circulation keeps working until its own TTL expires; nothing needs to be reissued.

v2 carries the same fields as v1 in a much smaller space. Measured on the same fully loaded invite:

| | v1 | v2 |
|---|---:|---:|
| every field present | 843 chars | 515 chars |
| relay and rendezvous are the defaults | — | **372 chars** |

Three things account for the difference:

- **Multiaddrs travel as bytes.** libp2p's binary encoding of `/ip4/…/tcp/…/p2p/…` is about 47 bytes; the same address spelled as text is about 84, because the peer ID gets re-encoded in base58. An address that will not parse falls back to text, so a hand-written `--peer` value still survives.
- **The grant travels as bytes.** `inviter_pubkey`, `nonce` and `sig` are a key, a UUID and a signature — 116 bytes. v1 carried them as hex and a UUID string: 236.
- **The default servers travel as a flag.** See below.

### Default relay and rendezvous

A stock build points both `DEFAULT_RELAY` and `DEFAULT_RENDEZVOUS` at the same host, so an invite that names them is spending ~190 bytes on an address the recipient's own binary already knows. When the address being embedded is the one resolving that default would produce, v2 sets a flag instead and carries no address; `enox enter` resolves it on the way in, the same way the daemon does at startup.

The match is judged on host, transport and port — not the peer ID, which the joiner fetches fresh anyway. A server on the default host but a different port is somebody's own deployment and is embedded in full. When the check is unsure it embeds the address, so the failure mode is a longer link, never a wrong one.

A self-hosted relay or rendezvous server is always carried explicitly.

---

## Binary format (v2)

The opaque payload is a variable-length byte array encoded as base64url (no padding), prefixed with `enoxian://v2/`.

### Fixed header (53 bytes)

| Bytes | Content |
|-------|---------|
| 0 | Flags (see below) |
| 1–16 | Circle UUID (per `Uuid::as_bytes()`) |
| 17–48 | PSK (32 raw bytes) |
| 49–52 | Expiry — Unix timestamp as `u32` big-endian (so no invite can expire after 2106-02-07; `parse_ttl` refuses a longer TTL) |

### Flags

| Bit | Meaning |
|-----|---------|
| `0x01` | Circle name follows |
| `0x02` | Peer address follows |
| `0x04` | Admin public key follows |
| `0x08` | Explicit relay address follows |
| `0x10` | Relay is `DEFAULT_RELAY` — no address carried |
| `0x20` | Explicit rendezvous address follows |
| `0x40` | Rendezvous is `DEFAULT_RENDEZVOUS` — no address carried |
| `0x80` | Grant follows |

`0x08`/`0x10` are mutually exclusive, as are `0x20`/`0x40`.

### Body

Only the flagged fields appear, in this order.

| Field | Encoding |
|-------|----------|
| Circle name | length + UTF-8 |
| Peer address | address field |
| Admin public key | length + raw bytes (Ed25519, protobuf-encoded) |
| Relay address | address field |
| Rendezvous address | address field |
| Grant | length + inviter pubkey, then nonce, then length + signature |

A **length** is an unsigned LEB128 varint: one byte below 128, which covers every field in practice, and a second byte beyond that. A multiaddr built on a long DNS name can exceed 255 bytes, and v1 carried those, so a fixed `u8` would have been a regression.

An **address field** is a tag byte — `0` for libp2p's binary multiaddr encoding, `1` for UTF-8 text — followed by a length and the bytes.

A **nonce** is a tag byte — `0` for 16 raw UUID bytes, `1` for UTF-8 text — followed by the bytes. `sign_grant` always produces a UUID; the text form exists so a grant minted elsewhere is not silently corrupted.

---

## Binary format (v3)

v3 is v2 with a second flags byte straight after the first, prefixed with `enoxian://v3/`. It is minted only by a Circle on the Iroh transport, so a libp2p Circle's invite stays v2 and older clients keep reading it.

| Bit | Name | Meaning |
|-----|------|---------|
| 0x01 | `FLAG2_IROH` | The Circle runs on Iroh. Always set on v3. |
| 0x02 | `FLAG2_IROH_RELAYS` | A list of Iroh relay URLs follows the grant |

The header and body are v2's. When `FLAG2_IROH_RELAYS` is set, a `u8` count and that many length-prefixed UTF-8 URLs follow the grant; the joiner stores them as `iroh_relays`. With no list, the Circle uses Iroh's public relays.

The peer address is a v2 address field. An Iroh peer is written `/ip4/…/udp/…/quic-v1/p2p/<peer id>`: the peer id is the Circle key the joiner dials by, and the IP and port a direct-address hint. A device with no address to offer writes `/p2p/<peer id>` alone and is reached through the relays; Iroh has no rendezvous server, so an Iroh invite always names a peer.

`enox enter` saves the joined Circle with `transport = "iroh"` and the relay list. A v2 or v1 invite leaves the transport to `ENOXIAN_TRANSPORT` and otherwise libp2p.

---

## Binary format (v1, decode only)

### Fixed header (58 bytes minimum)

| Bytes | Content |
|-------|---------|
| 0–15 | Circle UUID (big-endian, per `Uuid::as_bytes()`) |
| 16–47 | PSK (32 raw bytes) |
| 48–55 | Expiry — Unix timestamp as `i64` big-endian |
| 56 | Circle name length N (u8) |
| 57..57+N | Circle name (UTF-8, N bytes) |
| 57+N | Peer addr length M (u8) |
| 58+N..58+N+M | Peer multiaddr (UTF-8, M bytes) |

### Extensions (appended after peer addr, backward-compatible)

Extensions use a u16 big-endian length prefix. Old decoders that don't know about an extension read its length and skip it. A zero-length extension means the field is absent.

| Offset | Length | Content |
|--------|--------|---------|
| ext1 | u16 BE | Admin public key length (0 = absent) |
| ext1+2 | len | Admin public key (Ed25519, protobuf-encoded) |
| ext2 | u16 BE | Relay addr length (0 = absent) |
| ext2+2 | len | Relay multiaddr (UTF-8, TCP — e.g. `/ip4/1.2.3.4/tcp/36521/p2p/<id>`) |
| ext3 | u16 BE | Rendezvous addr length (0 = absent) |
| ext3+2 | len | Rendezvous server multiaddr (UTF-8, QUIC — e.g. `/ip4/1.2.3.4/udp/36521/quic-v1/p2p/<id>`) |
| ext4 | u16 BE ×3 | Grant: inviter pubkey hex, nonce, signature hex — each UTF-8 |

---

## Short-link storage

### What the relay sees

Nothing it can use. The blob id is an HKDF output of the key, so the id it is asked for says nothing about the key; the body is sealed under a second output of the same key, with the id bound in as associated data so a blob cannot be moved to another id. An operator learns that an invite exists, its size, and when it is fetched — not the circle, not the PSK, not who it is for.

The key is in the link. Anyone holding the link holds the invite, exactly as with the long form, so share it the same way.

### Which relay

The link names a relay only when it is not the build's default — that is what keeps it to 35 characters. A circle on a self-hosted rendezvous server gets `…@host:port` appended, which is still a fraction of the long form:

```
enoxian://s1/G9Qg1TN8zxAV2hHofrH0vA@pair.example.com:36521
```

`--rendezvous` aims both at once: the server the invite embeds for connectivity is the server its contents are left on.

### Limits

- **Blobs are kept 30 days.** Invites with a longer TTL use the self-contained form instead. A blob whose invite has expired is swept on the next write.
- **Write-once.** A link cannot be repointed at different contents once shared.
- **Fetching does not consume.** One-use is enforced where it means something: the grant nonce, burned when the circle admits the joiner.
- **A short invite needs the relay at redemption time.** Use `--long` when the recipient may not be able to reach it, or when the link has to work with no third party involved.
- **A TTL longer than 30 days is not shortened.** The relay only keeps a blob that long, and a link printed as valid for 90 days that stops resolving on day 31 would be worse than a long one. `enox invite --ttl 90d` prints the self-contained form and says why.
