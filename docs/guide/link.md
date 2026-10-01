# Linking your devices

Use `enox link` to add a second device under your existing user identity and
join your Circles. Install enoxian on both machines first.

## Pair the devices

On a device that already has your identity:

```sh
enox link
```

It prints a four-word code valid for two minutes. On the new device, run the
command it shows, for example:

```sh
enox link atom cargo salsa civic
```

Both screens show a six-digit confirmation number. Compare them before answering
`y` on each device. If the numbers differ, do not approve; start again.

After linking, run this on the new device:

```sh
enox start
enox identity show
enox circles
```

Each Circle applies its own admission policy, so joining can still require an
admin's approval. Your new device has its own device identity associated with
you; adding it does not require entering your recovery phrase.

## Pair another device

Any linked device can normally link the next one. A code pairs one device;
run `enox link` again for each additional machine.

Linking chains are limited to four hops from the user identity. If you reach
that limit, pair from a device closer to the original identity. A device that
already belongs to a different user identity will not be silently reassigned.

## Use your own pairing server

Both devices must use the same server:

```sh
enox link --server pair.example.com
enox link atom cargo salsa civic --server pair.example.com
```

Any `enox bootstrap serve` instance provides pairing. See
[hosting a relay](rendezvous-setup.md) for deployment. The server can observe
pairing activity and timing, but the exchanged identity data is encrypted.

## Recover without another device

The recovery phrase is shown when you create your identity and is not saved
for later display. Store it privately when it is created.

If you no longer have a working device, recovery uses:

```sh
enox identity link-user "<your-handle>" "<24-word-recovery-phrase>"
```

Use a trusted local terminal: the phrase is sensitive and may be retained in
shell history. Whenever a working device is available, prefer `enox link`.

Older installations may have saved a phrase locally. `enox identity show`
reports this. After preserving your own private copy, use:

```sh
enox identity forget-phrase
```

The command asks before removing the saved phrase. The device keeps its
identity and can still link new devices.

See [privacy and security](../concepts/security.md) for the broader access
model, or the [linking protocol](../development/reference/device-linking.md)
for implementation details.
