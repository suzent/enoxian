# Releasing enoxian

The public GitHub repository is the source of truth for source code, tags,
release notes, installers, checksums, and prebuilt binaries.

## CI jobs and branch protection

The CI workflow defines separate jobs so GitHub branch protection can require
them independently:

- `Rust quality`: formatting and Clippy with warnings denied.
- `Rust tests (ubuntu-latest)`.
- `Rust tests (macos-latest)`.
- `Rust tests (windows-latest)`.
- `Frontend production build`.
- `Dependency security audit`.
- `Installer syntax`.

Require all seven checks on `main`, require pull requests, and block force
pushes. A tag push does not inherit branch CI, so the Release workflow repeats
the essential release gate and dependency audit before packaging.

## Release workflow

1. From a clean, current `main`, run `scripts/bump.sh patch` or
   `scripts/bump.ps1 patch`. It creates a `chore/release-vX.Y.Z` branch, cuts the
   CHANGELOG section, updates Cargo files, and commits without tagging or
   pushing.
2. Push the branch and merge its PR only after required CI passes.
3. Update local `main`, then create and push the signed tag printed by the bump
   script.
4. Release automation verifies tag/version/CHANGELOG consistency and confirms
   that the tag commit is reachable from `origin/main`.
5. All five platforms build into temporary workflow artifacts. Native binaries
   are smoke-tested with `--version`.
6. Only after every build succeeds does one publish job create a draft, upload
   all assets plus `SHA256SUMS` and the two installers, then publish it. A failed
   upload remains a draft instead of exposing a partial release.

Published releases are immutable: rerunning a completed release refuses to
overwrite an already-public tag.

## Optional Homebrew tap

The GitHub Release itself needs no additional repository. A named Homebrew tap
does: set repository variable `ENOXIAN_HOMEBREW_TAP` (for example
`suzent/homebrew-tap`) and secret `ENOXIAN_HOMEBREW_TOKEN` only if that install
channel is wanted.

After publishing, the optional job downloads the final macOS/Linux assets,
updates the Formula version and SHA256 values, runs `brew audit`, installs and
tests the Formula, then commits it to the tap. If the variable is unset, the job
is skipped and GitHub Release installation remains fully functional.
