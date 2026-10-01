# Privacy and security

## Choose your Circle members carefully

Members can read shared Circle content. Keep private files outside the shared
folder, and share invite links through a trusted private channel. Anyone with
an invite has the information needed to attempt joining; admission follows the
Circle's membership policy.

An invite's expiry limits onboarding through the CLI. It does not remove an
existing member or erase copies of files they already received. Generating a
new invite does not invalidate previous links. See [invites](../guide/invite.md).

## Decide which agents can run

Agent configuration is local to each device. Enabling `push` lets Circle
mentions launch the agents you have configured there. Keep only agents you
intend Circle members to use, and review their own tool permissions.

Agents may send messages, files, and context to their model providers. Ambient
agents read unaddressed human messages as well, so enabling them changes what
is sent outside the Circle. See [using agents](../guide/agents.md).

## Relay and rendezvous

Circle content is encrypted in transit, including when it passes through a
relay. The project-operated default service is `relay.enoxian.com`. Its
operator can observe connection metadata such as peer IDs, network addresses,
and timing, but cannot read the encrypted Circle content merely by relaying it.

You can [host your own relay](../guide/rendezvous-setup.md) and configure your
Circles to use it.

## Protect local data

Shared files and persisted workspace history are not encrypted at rest by
enoxian. Protect them with device access controls, disk encryption, and suitable
backups. Some stored secrets use the operating system's credential facilities;
that does not encrypt the shared folder or all local state.

The local daemon API is for trusted local clients. Do not expose it publicly
without understanding its authentication and network configuration.

## Link and remove devices deliberately

When [linking a device](../guide/link.md), compare the confirmation number on
both screens before approving. Keep your recovery phrase private.

Removing a member blocks future authorized synchronization; it cannot delete
content already copied elsewhere. Use the
[membership commands](../guide/cli.md#members) to inspect and manage access.

For the technical trust model and remaining limitations, see the
[developer security reference](../development/architecture/security.md). To report a
vulnerability, follow [SECURITY.md](../../SECURITY.md).
