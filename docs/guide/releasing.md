# Releasing enoxian

The public GitHub repository is the source of truth for source code, tags,
release notes, installers, checksums, and prebuilt binaries.

Cutting a release takes two actions, both in the browser:

1. **Actions → Prepare release → Run workflow**, choosing `patch`, `minor`, or
   `major`.
2. **Merge the release pull request** it opens, once required CI is green.

Everything after that is automatic: the merge is tagged, all five platforms are
built and attested, the release is published, the published installers are
exercised on three operating systems, and only then is the release marked
`latest`.

## CI jobs and branch protection

The CI workflow defines separate jobs so GitHub branch protection can require
them independently:

- `Workflow lint`: actionlint over `.github/workflows`.
- `Changelog entry`: the pull request touches `CHANGELOG.md`. Skipped for
  Dependabot and for pull requests labelled `no-changelog`.
- `Rust quality`: formatting, Clippy with warnings denied, and doctests.
- `Rust tests (ubuntu-latest)`.
- `Rust tests (macos-latest)`.
- `Rust tests (windows-latest)`.
- `Frontend production build`: `npm ci`, a production-dependency audit, build.
- `Dependency security audit`: `cargo audit`, reading its accepted-advisory list
  from `.cargo/audit.toml`.
- `Installer syntax`: both installers parse, and no shell script has CRLF.
- `Bootstrap image`: builds `Dockerfile` and checks its entrypoint. On pull
  requests it only builds when an input to the image changed, so it is cheap
  enough to require as well.

Require these on `main`, require pull requests, and block force pushes. A tag
push does not inherit branch CI, so the Release workflow repeats the essential
release gate and dependency audit before packaging anything.

CI also runs on `merge_group`, so the merge queue can be enabled without any
further change.

## Preparing a release

`Prepare release` (`.github/workflows/prepare-release.yml`) runs
`scripts/bump.sh` on a runner. The script:

- refuses to run unless it is on a clean `main` that is current with
  `origin/main` — a stale `main` would silently leave newly merged CHANGELOG
  entries under `[Unreleased]` while shipping their code;
- refuses to run when `[Unreleased]` is empty, since that section becomes the
  release notes;
- creates `chore/release-vX.Y.Z`, cuts the dated CHANGELOG section, updates the
  compare links, sets the version in `Cargo.toml`, `Cargo.lock`, and the README,
  and commits.

It then pushes the branch. If the `ENOXIAN_RELEASE_TOKEN` secret is set (a
fine-grained PAT with Contents and Pull requests write), the job also opens the
pull request. Without that secret it prints a one-click link in the job summary
instead — pull requests opened by `GITHUB_TOKEN` deliberately do not trigger
workflows, so required checks would never report and the branch could not merge.

You can still run `scripts/bump.sh` or `scripts/bump.ps1` locally; the workflow
is only a convenience wrapper around it.

## Tagging

When a `chore/release-v*` pull request merges, `tag-release.yml` cross-checks
the branch name against `Cargo.toml` and the CHANGELOG, creates an annotated tag
on the merge commit, pushes it, and dispatches the Release workflow. (A tag
pushed with `GITHUB_TOKEN` does not itself trigger workflows, so the dispatch is
explicit.)

Tags are created by CI and are therefore not GPG-signed. Release integrity is
established instead by signed build provenance — see
[Verifying a release](#verifying-a-release).

## The release pipeline

`.github/workflows/release.yml` runs six stages. Each one must pass before the
next begins:

1. **`verify`** — the tag is well-formed, matches `Cargo.toml`, is the
   checked-out commit, is reachable from `origin/main`, and has a non-empty
   CHANGELOG section. Then the full Rust, frontend, and installer gate runs.
   Any suffix on the version (`v0.5.0-beta.1`) marks the release a prerelease,
   which is never promoted to `latest`.
2. **`audit`** — `cargo audit` against the tagged tree.
3. **`build`** — all five targets build into workflow artifacts. The
   linux-x86_64, macos-aarch64, and windows binaries are smoke-tested: version,
   help output, then a real daemon start, an embedded WebUI fetch, and a clean
   `enox stop`. Each archive gets signed build provenance.
4. **`publish`** — assets are uploaded to a *draft* so a partial upload never
   becomes visible, then the release goes live **as a prerelease**. At this
   point `/releases/latest/download`, which both installers use by default,
   still resolves to the previous good release.
5. **`install-smoke`** — on Linux, macOS, and Windows: download the published
   installer, install by explicit tag, check the version, run
   `enox service status`, verify the archive's provenance, then re-run the
   installer while a daemon is live to confirm the upgrade path stops it.
6. **`promote`** — marks the release `latest`. Only now do `curl | sh` users
   move onto it.

The optional `homebrew` job then updates the tap.

Published releases are immutable: rerunning a completed release refuses to
overwrite an already-promoted tag.

## Verifying a release

Both installers verify the downloaded archive against `SHA256SUMS` before
installing. Because that file is served from the same place as the archives, it
protects against corruption rather than against a compromised release — so every
archive also carries signed, transparency-logged build provenance tying it to
this repository, this workflow, and this commit:

```sh
gh attestation verify enoxian-macos-aarch64.tar.gz --repo suzent/enoxian
```

The release pipeline runs that same check against the published artifacts before
promoting a release.

## Rolling back a bad release

Installers resolve `/releases/latest/download` by default, so `latest` is the
thing to move — not the tag, and not the assets.

**Mark the bad release as a prerelease.** GitHub then re-points `latest` at the
previous release, and new installs go back to it immediately:

```sh
gh release edit v0.4.3 --repo suzent/enoxian --prerelease --latest=false
```

Then fix forward with a normal patch release. Do not delete the release: the tag
would remain, and anyone pinned with `--version v0.4.3` would get a 404 instead
of a working older binary.

If `install-smoke` fails, no rollback is needed — the release is still a
prerelease and was never promoted. Investigate, then either fix forward or
delete the unpromoted release.

## Prereleases

Tag a version with a suffix (`0.5.0-beta.1` in `Cargo.toml`, tag `v0.5.0-beta.1`)
and the pipeline publishes it, smoke-tests it, and stops — it is never marked
`latest`, and the Homebrew tap is not updated. Users opt in explicitly:

```sh
curl -fsSL https://github.com/suzent/enoxian/releases/latest/download/install.sh \
  | sh -s -- --version v0.5.0-beta.1
```

## Optional Homebrew tap

The GitHub Release itself needs no additional repository. A named Homebrew tap
does: set repository variable `ENOXIAN_HOMEBREW_TAP` (for example
`suzent/homebrew-tap`) and secret `ENOXIAN_HOMEBREW_TOKEN` only if that install
channel is wanted.

After a release is promoted, the optional job downloads the final macOS/Linux
assets, updates the Formula version and SHA256 values, runs `brew audit`,
installs and tests the Formula, then commits it to the tap. If the variable is
unset, the job is skipped and GitHub Release installation remains fully
functional.
