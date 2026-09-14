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

- **A linked device cannot link a third.** Only the device you ran `enox identity create-user` on stores the mnemonic, so only it can sign an attestation. Link every device from that one. A device without the root key can still pass its circles over — it says so when it does, and the new device joins the circles without being attested.
- **Circles are joined, not yet admitted.** The new device writes a pending entry carrying the invite grant, exactly as `enox enter` does. It is admitted under each circle's own join policy.
- **Invites in a link payload last 24 hours.** They are meant to be redeemed within seconds; the window is slack for a machine paired now and started later.
- **One code, one device.** A code admits a single offer. Run `enox link` again for another machine.

---

## Recovering without another device

If every device is gone, the mnemonic from `enox identity create-user` is the way back:

```bash
enox identity link-user "suzy" "<24 words>"
```

That path is disaster recovery. `enox link` is the one to use whenever you still have a working device — it does not put the root key on a clipboard.
