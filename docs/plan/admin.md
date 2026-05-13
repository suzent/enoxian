# Admin & Member Management — Design Plan (M6)

## Why the current model is unsafe

ENOCHIAN currently uses a single shared PSK as the only membership credential. This means:

- **No invite gating** — any peer can generate a valid invite (they all hold the PSK)
- **No revocation** — removing a member requires rotating the PSK and manually re-distributing the new secret to every remaining member out-of-band
- **No permissions** — the CRDT merges all writes from all peers equally; there is no read-only or restricted role
- **One leak = permanent breach** — a single leaked invite gives permanent, irrevocable membership

This is acceptable for small, fully-trusted teams where everyone personally knows every other member. It is not acceptable for any other use case.

---

## Design

### Core idea: admin keypair + signed member list

Each circle has an **admin keypair** (separate from any node's identity keypair). The admin keypair signs a member list stored in the control CRDT doc. Every peer verifies this list on connect.

```
Circle
  ├── PSK                     transport filter (can you reach the swarm)
  ├── Admin keypair           signs the member list
  └── Member list (CRDT)      {peer_id, role, added_at, signature}
        ├── peer-id-A  → admin
        ├── peer-id-B  → member
        └── peer-id-C  → observer (read-only)
```

### Roles

| Role | Can sync files | Can write tasks/locks | Can generate invites | Can manage members |
|------|---------------|----------------------|---------------------|-------------------|
| Admin | ✓ | ✓ | ✓ | ✓ |
| Member | ✓ | ✓ | ✗ | ✗ |
| Observer | ✓ (read) | ✗ | ✗ | ✗ |

### Invite flow (with admin)

1. Admin runs `enoch invite MyCircle` — invite is signed with admin private key
2. Joiner runs `enoch enter <uri>` — verifies admin signature before saving config
3. On connect, joiner's peer ID is checked against the member list
4. If not in the list → connection refused

### Revocation flow

1. Admin runs `enoch member remove <peer-id>`
2. Admin signs updated member list (without the revoked peer)
3. Updated list propagates via CRDT to all peers
4. All peers refuse future connections from the removed peer ID

The removed peer's existing connections may linger briefly until they reconnect. PSK rotation is not required.

### Admin key storage

The admin private key is stored separately from the node keypair:

```
~/.enochian/circles/<id>/
  config.toml       — node keypair, PSK, workspace_dir
  admin.key         — admin private key (only on admin machines)
```

`admin.key` is never shared. Admin authority can be transferred by signing a new admin key with the current one.

---

## Config changes

```toml
# config.toml additions
admin_pubkey_hex  = "..."     # admin public key (all peers store this)
member_role       = "member"  # this node's role: admin | member | observer
```

```
# admin.key (admin machines only, not in config.toml)
# Raw Ed25519 private key, hex-encoded
```

---

## CLI changes

```bash
# List all members
enoch member list

# Remove a member (admin only)
enoch member remove <peer-id>

# Promote to admin (admin only)
enoch member add-admin <peer-id>

# Change a member's role
enoch member set-role <peer-id> observer
```

---

## Migration from shared-PSK model

Circles created before M6 use the PSK-only model. Migration:

1. Admin runs `enoch upgrade-circle MyCircle` (generates admin keypair, signs current member list)
2. All peers update via CRDT sync
3. New connections require member list check

Old clients (without M6) can still connect via PSK — but their writes may be rejected by M6+ peers if they're not in the member list. A compatibility flag in the control doc signals whether a circle requires member-list enforcement.

---

## Implementation tasks

- [ ] Generate admin keypair at `enoch init`, store in `admin.key`
- [ ] Store `admin_pubkey_hex` in `config.toml`
- [ ] Sign member list entries with admin key
- [ ] `enoch enter` — verify admin signature on invite, add self to pending member list
- [ ] On `ConnectionEstablished` — verify peer is in member list, disconnect if not
- [ ] `enoch member list` command
- [ ] `enoch member remove` command (admin only)
- [ ] `enoch member add-admin` command (admin only)
- [ ] `enoch member set-role` command (admin only)
- [ ] Observer enforcement — reject CRDT writes from observer peers
- [ ] `enoch upgrade-circle` migration command
