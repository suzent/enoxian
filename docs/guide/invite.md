# Inviting people to a Circle

Generate an invite on a device that already belongs to the Circle:

```sh
enox invite MyCircle
```

Share the link privately. On the other device:

```sh
enox enter "<invite-link>"
enox start
```

Use `enox --circle MyCircle who` to check connected participants. Joining may
require approval under the Circle's membership policy; an admin can inspect
requests with `enox member pending`.

For your own second device, [device linking](link.md) also transfers your user
identity and invites for your Circles.

## Choose an expiry

Invites default to seven days. For a shorter joining window:

```sh
enox invite MyCircle --ttl 24h
```

`--ttl` accepts whole hours (`1h`, `24h`) or days (`7d`, `90d`), within the
supported timestamp range. Creating a new invite does not invalidate old ones.
Expiry prevents new joins through `enox enter`; it does not disconnect existing
members or revoke a leaked secret.

## Short and self-contained links

`enox invite` normally prints a short link beginning with `enoxian://s1/`.
The recipient needs to reach the relay to retrieve its encrypted contents.

For a link carrying its own contents:

```sh
enox invite MyCircle --long
```

This prints an `enoxian://v3/` link. It does not require the short-link service
to resolve, though joining and syncing still require connectivity to the Circle.
Older `v1` and `v2` links remain readable until they expire.

If the relay cannot store a short invite, or the requested TTL is longer than
its 30-day retention, enoxian prints a self-contained link with an explanation.
Fetching a short link does not consume it, but admission grants can be consumed
when a device joins. Generate another invite if a grant has already been used.

## Connect across networks

Start with a normal invite: enoxian includes available connection information
automatically. Devices find each other through relays: by default the
project-operated relay plus Iroh's public relays. They connect directly when
the network allows it, and otherwise stay on the relay.

If your team runs its own server for short links and pairing:

```sh
enox invite MyCircle --rendezvous enox.example.com
```

For advanced setups, `--peer` supplies a direct peer address and `--relay`
names an Iroh relay URL (such as `https://relay.example.com`) for the invite. See the [CLI reference](cli.md#invite) and
[relay deployment guide](rendezvous-setup.md) for details.

## Share safely

Treat both short and long links as credentials. Send them through a trusted
private channel, and keep them out of public chat and version control.
The long form is encoded, not encrypted; the short form carries the key needed
to open its stored contents.

Read [privacy and security](../concepts/security.md) for membership, agent
access, and relay privacy. Implementers can find the
[invite formats](../development/reference/invites.md) in the developer docs.
