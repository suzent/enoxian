<!--
Keep each pull request focused and explain the user-visible behavior.
See CONTRIBUTING.md.
-->

## What changes for users

<!-- One or two sentences describing the effect, not the diff. -->

## Checklist

- [ ] `cargo fmt --all -- --check`, `cargo clippy --locked --all-targets -- -D warnings`,
      and `cargo test --locked --all-targets` pass locally (or `scripts/pre-push.sh`).
- [ ] Behavior changes have tests.
- [ ] User-visible changes are listed under `## [Unreleased]` in `CHANGELOG.md`
      (apply the `no-changelog` label if users cannot observe this change).
- [ ] Frontend changes: `npm ci && npm run build` in `frontend/` passes.
