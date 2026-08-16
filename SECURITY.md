# Security Policy

## Supported versions

Security fixes are provided for the latest published release. Users should
upgrade to the newest release before reporting an issue that may already be
fixed.

## Reporting a vulnerability

Please do not open a public issue for a suspected vulnerability. Use
[GitHub's private vulnerability reporting](https://github.com/suzent/enoxian/security/advisories/new)
to send a description, reproduction steps, affected versions, and any proposed
fix. If private reporting is unavailable, contact the maintainers privately
through their GitHub profiles and ask for a secure reporting channel.

We aim to acknowledge reports within 3 business days, provide an initial
assessment within 7 business days, and coordinate disclosure after a fix is
available. These are targets rather than guarantees.

## Scope notes

enoxian is local-first peer-to-peer software. Its transport and local API
security model, trust boundaries, and current limitations are documented in
[docs/concepts/security.md](docs/concepts/security.md). In particular, read
that document before relying on enoxian for sensitive data.
