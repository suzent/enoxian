# Contributing to enoxian

Thanks for helping improve enoxian. Bug reports, focused feature proposals,
documentation fixes, and pull requests are welcome.

## Development setup

Install Rust 1.88 or newer. Node.js 22 is required only for frontend work.

```sh
git clone https://github.com/suzent/enoxian.git
cd enoxian
cargo test --locked --all-targets
cd frontend && npm ci && npm run build
```

Before opening a pull request, run:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```

If frontend dependencies or source changed, also run `npm audit` and
`npm run build` in `frontend/`.

## Pull requests

- Keep each pull request focused and explain the user-visible behavior.
- Add tests for behavior changes and update documentation where needed.
- Add user-visible changes under `Unreleased` in `CHANGELOG.md`.
- Do not commit credentials, Circle secrets, invite URLs, local state under
  `~/.enoxian`, build output, or generated frontend assets.

The release process is documented in
[docs/guide/releasing.md](docs/guide/releasing.md). Security issues follow
[SECURITY.md](SECURITY.md), not the public issue tracker.
