# Getting started

Install enoxian, create a Circle, and share your first note across devices.
You can use the web UI or the `enox` CLI throughout.

## 1. Install

**macOS / Linux**

```sh
curl -fsSL https://github.com/suzent/enoxian/releases/latest/download/install.sh | sh
```

**Windows PowerShell**

```powershell
irm https://github.com/suzent/enoxian/releases/latest/download/install.ps1 | iex
```

The installers choose the release for your platform and verify its checksum.
On Windows the install directory is `%LOCALAPPDATA%\enoxian\bin`; reopen your
terminal if `enox` is not yet on `PATH`.

To choose a version or installation directory on macOS / Linux:

```sh
curl -fsSL https://github.com/suzent/enoxian/releases/latest/download/install.sh | \
  sh -s -- --version <release-tag> --bin-dir "$HOME/.local/bin"
```

On Windows, set `$env:ENOXIAN_VERSION` to the release tag before running the
installer. Source builds are covered in the
[developer guide](../development/contributing/dev-guide.md).

## 2. Create a Circle

```sh
enox init --name MyCircle
enox start
enox open
```

`init` prints the location of your shared folder, normally
`~/enoxian/MyCircle`, and an invite. `start` runs enoxian in the background;
`open` opens the local web UI.

Add a note inside that folder using your usual editor. This is where shared
notes, decisions, and hand-offs live. Keep repositories, dependencies, and
build output elsewhere; record their device and path in a shared note.

To choose a different shared folder at creation time:

```sh
enox init --name MyCircle --dir ~/shared/MyCircle
```

## 3. Connect another device

On the first device:

```sh
enox invite MyCircle
```

Share the resulting link privately. On the other device, install enoxian and run:

```sh
enox enter "<invite-link>"
enox start
enox open
```

Admission follows the Circle's membership policy; a request may need an admin's
approval. Once connected, your shared note should appear on the other device.

Use [device linking](link.md) instead when adding another device under your own
identity. See [invites](invite.md) for expiry and connection options.

## 4. Find your way around

Use the web UI for chat, tasks, files, and history. From a terminal:

```sh
enox status
enox who
enox tasks
```

With multiple Circles, specify which one you want:

```sh
enox circles
enox --circle MyCircle status
enox --circle MyCircle open
```

You can also set `ENOXIAN_CIRCLE` in your shell. One background daemon handles
all enabled Circles on this device.

## Keep enoxian running after login

```sh
enox service install
enox service status
```

The login service restarts enoxian if it exits unexpectedly. Installing a
service does not enable agent execution; that is a separate setting in the
[agent guide](agents.md).

## If something does not connect

- **Cannot reach the daemon:** run `enox start`, then `enox status`.
- **Unsure which Circle is selected:** run `enox circles` and pass `--circle`.
- **Invite expired:** ask the sender to generate a new link.
- **Waiting for admission:** ask a Circle admin to check `enox member pending`.
- **Peer missing:** check `enox who` on both devices and confirm both are running.
  Devices on different networks may need a relay; see [invites](invite.md).

By default, enoxian can use the project-operated relay to help devices connect.
Read [privacy and security](../concepts/security.md) for what that service sees.

## Next steps

- [Everyday collaboration](collaboration.md)
- [Set up an agent](agents.md)
- [Review file changes](../concepts/proposals.md)
- [Find a command](cli.md)
