# Admin And Member Management

**Status:** Complete. This M6 design has been implemented and summarized in
[milestones.md](milestones.md#m6--admin-and-member-management).

Current authoritative references:

- Member and admin security model: [../../security.md](../../security.md)
- CLI commands: [../../cli.md](../../cli.md)
- API endpoints: [../../api.md](../../api.md)
- Current roadmap: [../roadmap.md](../roadmap.md)

## What Shipped

- Admin keypair generation at `enox init`.
- `admin.key` stored only on admin machines.
- Admin public key embedded in invites.
- Admin-signed member operations.
- Member list replicated in the control CRDT.
- Pending member approval/rejection flow.
- `enox member list/add/remove/promote/pending/approve/reject`.
- Daemon auto-signing for local frontend operations when `admin.key` is present.
- Stale pending and ghost member cleanup.
- Member removal integrated with MLS membership state and the `mls_removed`
  tombstone sync gate.

## Notes

The original M6 plan described the member list as the primary gate. The current
model is stricter:

```text
stable transport PSK
  -> Noise peer identity
  -> signed member state
  -> mls_removed tombstone sync gate
```

The transport PSK is not rotated on every membership change. Future
cryptographic content revocation belongs to Layer 4 content encryption, tracked
in [roadmap.md](../roadmap.md#m17--layer-4-content-encryption).
