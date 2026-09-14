# Linking a Device

## Overview

`enox link` puts your identity on a second machine. Run it bare on the device that already has the identity, and with the code it prints on the new one.

```bash
enox link
```

```
✦ Linking a new device

  On the other machine, run:

      enox link atom cargo salsa civic

  Waiting for it to connect (the code is good for 2 minutes)…
```

On the new machine — as separate words, or quoted, whichever you prefer:

```bash
enox link atom cargo salsa civic
```

Both devices then show the same six-digit number. Compare them, answer `y` on each, and the new device is linked — with every circle already joined.

```
  Confirmation number: 099-661

  Does your other device show the same number? [y/N] y
  Linked to user 'suzy'.
  Joined 'alpha'.
  Joined 'beta'.

✦ This device is linked. 2 circles ready.
  Run `enox start` to connect.
```

---

## What crosses the wire

**Not your mnemonic.** The new device generates its own device key locally and never sends it. What it receives is:

| | |
|---|---|
| An attestation | Your user root key's signature over the device key the new machine made for itself |
| Your handle | So both devices present the same name |
| One invite per circle | Ordinary `enoxian://` invites, redeemed through the same path a pasted invite takes |

The user root key never leaves the device that holds it. The new device ends up with an identity of its own, which means it can be removed on its own — a copied root key could not be.

The receiving device checks the attestation against the key it generated for itself before writing anything, so an attestation that proves nothing never reaches disk. It also refuses to join a *different* user identity than one it already belongs to — moving a device between identities is not something to do by accident.

### Any linked device can link the next one

A device that was linked does not hold the root key, but it does hold a signature chain reaching back to it. Linking from there extends the chain by one link, signed with that device's own key:

```
user root ──signs──▶ laptop ──signs──▶ phone
```

`enox identity show` says how far a device sits from the root:

```
  attestation: valid (2 hops from your user key)
```

Verification walks the whole chain: every link must be signed by the key the previous link vouched for, the first must be signed by the user root, and the last must name the device presenting it. A key may appear once, so a chain cannot be padded with a cycle.

The root key still never moves. It signs the first link and nothing else.

Check the result with `enox identity show`, which verifies the signature rather than just reporting that one is present:

```
  user       : suzy
  user pubkey: 08011220426f7a4b…
  attestation: valid
```

---

## Why the number matters

The six digits are derived from an X25519 exchange between the two devices. Anyone who intercepted the code and tried to stand in the middle would establish a *different* shared secret, so their number would not match — and you would stop.

This is the same trade Bluetooth pairing, ZRTP and Matrix make: it turns a network attacker into someone who has to be standing at both of your machines. Signal's device linking shipped without this step and was later exploited with forged QR codes, which is why both devices ask, separately, before either acts.

**Do not answer `y` unless you are looking at both screens.** The code being secret is a second line of defence, not the first one.

---

## The pairing server

The two devices meet in a mailbox on the bootstrap server — the one address both machines already know how to reach. Point them elsewhere with `--server`:

```bash
enox link --server pair.example.com
enox link atom cargo salsa civic --server pair.example.com
```

Both sides must name the same server. Any `enox bootstrap serve` provides the mailbox.

The server is trusted with nothing:

- The mailbox id is an HKDF output, not the code, so the server never sees the code.
- The offer is sealed under a key derived from the code, so the operator cannot read the new device's key or hostname.
- The payload is sealed under the X25519 secret, which the server never has.
- A substituted message fails the confirmation number.
- Pairing requests carry no credentials. The CLI's usual HTTP client sends the local daemon's bearer token on every request; the mailbox is deliberately given a separate client with no ambient credentials, so a pairing server never sees one.

An operator learns that two devices paired, and roughly when. Mailboxes are write-once per slot, capped in size and number, and expire after two minutes. Each source gets 60 requests per 10 seconds — about four times what a real pairing needs — counted per IPv4 address and per IPv6 `/64`.

Behind a reverse proxy that limit does nothing, because every client arrives as the proxy. `X-Forwarded-For` is deliberately not consulted: trusting it by default would let anyone set it to whatever they liked.

---

## About the code

Four words from the [EFF short wordlist](https://www.eff.org/dice) — 1296 words of three to five letters, no two sharing a prefix. That is about 41 bits.

Deliberately **not** BIP-39 words, even though those are already in the build. BIP-39 is what your recovery phrase is made of, and the whole point of `enox link` is that the phrase never has to be typed into a prompt again. Four recovery-shaped words at a pairing prompt would teach exactly the habit that makes the real phrase phishable.

41 bits is sized against *online* guessing: every guess costs an attacker one request to the mailbox, and the mailbox exists for two minutes. Reaching even a 1% chance inside one pairing would take on the order of 10¹⁰ requests in that window — far past what one server answers. The confirmation number, not the code, is what stops a man in the middle.

Words are separated by spaces. One word on the list — `yo-yo` — contains a hyphen, so hyphens join rather than split; spaces, dots and commas all separate, and case is ignored.

---

## Limits

- **Chains have a depth limit.** A device four hops from the user root cannot link another; link that one from a device closer to the root. In practice nothing gets near this.
- **Circles are joined, not yet admitted.** The new device writes a pending entry carrying the invite grant, exactly as `enox enter` does. It is admitted under each circle's own join policy.
- **Invites in a link payload last 24 hours.** They are meant to be redeemed within seconds; the window is slack for a machine paired now and started later.
- **One code, one device.** A code admits a single offer. Run `enox link` again for another machine.
- **A device belongs to one identity.** Linking a device that already belongs to a different user is refused rather than silently reassigned.

---

## Proving who a device belongs to

Inside a circle, a peer publishes an owner claim. Until now that was a name and a signature over it — which proves somebody holds the peer's key, not that they are who the name says. Any peer could write `owner: "suzy"`.

A linked device now publishes three things alongside the name:

| | |
|---|---|
| Its device key | The key its attestation chain reaches |
| A binding | Signed with the circle key, over the device key — ties the peer ID to that device |
| Its attestation chain | Carries the user root's authority down to the device key |

The binding is needed because a per-circle key is HKDF-derived from the device seed, so a peer ID and a device key have no arithmetic relationship — without a signature there is nothing linking them. It is signed by the **device** key over the circle key, so producing one needs that device's private key. Signed the other way round it would prove nothing: every peer holds its own circle key, and a device key, user key and chain are all published, so any member could pair someone else's public material with a binding of their own.

`enox member list` marks a name nobody can check:

```
  [admin] 12D3KooWAbc…  owner=suzy
  [member] 12D3KooWXyz…  owner=suzy (unverified)
```

Two devices belong to the same person when they prove the same user key — not when they happen to write the same name. A peer that was never linked to a user shows as unverified, which is the honest answer for it; nothing is rejected on this basis yet.

---

## Recovering without another device

The recovery phrase is shown once, when you create the identity, and **is not saved anywhere**. Write it down then; nothing will show it to you again.

It is not needed to add devices — `enox link` from any device you are already signed in on does that, using that device's attestation. The phrase is for the case where no device is left:

```bash
enox identity link-user "suzy" "<24 words>"
```

That path is disaster recovery. `enox link` is the one to use whenever you still have a working device — it does not put the root key on a clipboard.

### If an older install left one on disk

Before this, the phrase was kept in `identity.toml` on the device that created the identity, because only a device holding the root key could vouch for another. Attestation chains removed that need, and what was left was a phrase whose presence turned a lost laptop into a lost identity — permanently, since there is no revocation.

`enox identity show` says so when it finds one. It is not deleted for you: those words may be the only copy anyone has. When you have written them down:

```bash
enox identity forget-phrase
```

It shows them one last time, asks, and removes them. The device keeps its identity and can still link new ones.
