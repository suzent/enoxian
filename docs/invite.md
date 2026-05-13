# Invite Links

## Overview

Circles are joined via `enochian://` invite URIs — single strings that encode everything needed to authenticate and connect.

```
enochian://v1/CRxkUjpNEK2NavCQ8mNoxb2t7pQrAbbd1BKzMzD1c3JTpL4v?expires=2026-05-20T14:00:00Z&name=MyCircle
```

A link encodes:
- The circle UUID
- The pre-shared key (PSK)
- An expiry timestamp (required)
- The circle name (optional, human-readable label)
- A bootstrap peer address (optional, for WAN connections without mDNS)

---

## Binary format

The opaque payload (`CRxkUjpN...`) is 48 bytes encoded as base64url (no padding):

| Bytes | Content |
|-------|---------|
| 0–15 | Circle UUID (big-endian, per `Uuid::as_bytes()`) |
| 16–47 | PSK (32 raw bytes) |

Query parameters:

| Parameter | Required | Format | Notes |
|-----------|----------|--------|-------|
| `expires` | Yes | `YYYY-MM-DDTHH:MM:SSZ` | RFC 3339 UTC |
| `name` | No | percent-encoded string | Human label only |
| `peer` | No | base64url-encoded multiaddr | Avoids encoding `/` in query string |

---

## Generating invites

**At circle creation** — `enoch init` always prints a 7-day invite:

```bash
enoch init --name "MyCircle"
enoch init --name "MyCircle" --ttl 24h   # shorter window
```

**On demand** — `enoch invite` generates a fresh link without touching the circle config:

```bash
enoch invite <circle-id>                       # 7-day default
enoch invite <circle-id> --ttl 1h             # expires in 1 hour
enoch invite <circle-id> --peer /ip4/1.2.3.4/tcp/9091  # WAN-ready
```

Generating a new invite does **not** invalidate old ones — it just produces a different link with a new expiry. If you need to invalidate all outstanding invites, rotate the PSK (not yet implemented; requires re-issuing invites to all existing members).

---

## Expiry enforcement

Expiry is enforced **client-side** in `enoch enter`. Before dialing the network, the CLI checks `Utc::now() > expires_at` and exits with an error if the invite has lapsed:

```
Error: invite expired 2h ago (at 2026-05-13 12:00 UTC)
```

This means:
- Expired links cannot be used to accidentally join
- An agent that is already connected is not affected by expiry — only new joins are gated
- A malicious agent with a copy of the PSK can still connect if they bypass `enoch enter` (the PSK is not time-limited at the P2P layer)

A future approval-gate feature will enforce membership server-side inside the daemon, removing this limitation.

---

## Security properties

| Property | Status |
|----------|--------|
| Single string to share | ✅ |
| Expires automatically | ✅ (client-enforced) |
| Opaque — PSK not visible in plain text | ✅ (base64url encoded) |
| Works offline / without a server | ✅ |
| Revocable per-agent | ❌ (PSK rotation only) |
| Server-enforced expiry | ❌ (planned — approval gate) |
| One-time use | ❌ (reusable for the TTL window) |

---

## Sharing guidelines

- Share over a trusted, private channel (direct message, encrypted config file, secrets manager)
- Use short TTLs (`1h`, `24h`) for one-off agent onboarding
- Use `7d` or longer for persistent team setups where you need multiple members to join over time
- Do not commit invite links to version control or paste them in public channels

---

## TTL format

The `--ttl` flag accepts:

| Input | Meaning |
|-------|---------|
| `7d` | 7 days |
| `24h` | 24 hours |
| `1h` | 1 hour |
| `90d` | 90 days |

Any integer followed by `d` (days) or `h` (hours) is valid.
