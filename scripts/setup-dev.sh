#!/usr/bin/env bash
# Run once after cloning to wire up local dev tooling.
# Usage: ./scripts/setup-dev.sh
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"

echo "▶ Installing pre-commit hook..."
cp "$REPO/scripts/pre-commit.sh" "$REPO/.git/hooks/pre-commit"
chmod +x "$REPO/.git/hooks/pre-commit"
echo "  ✓ .git/hooks/pre-commit installed"

echo "▶ Installing pre-push hook..."
cp "$REPO/scripts/pre-push.sh" "$REPO/.git/hooks/pre-push"
chmod +x "$REPO/.git/hooks/pre-push"
echo "  ✓ .git/hooks/pre-push installed"

echo "▶ Checking rustfmt..."
rustup component add rustfmt clippy 2>/dev/null || true
echo "  ✓ rustfmt + clippy available"

echo "▶ Installing frontend deps..."
(cd "$REPO/frontend" && npm install --silent)
echo "  ✓ frontend deps installed"

echo
echo "Dev setup complete."
echo "The pre-commit hook (fast, changed files only) will run:"
echo "  - cargo clippy -- -D warnings  (on .rs / Cargo changes)"
echo "  - cargo test                   (on .rs / Cargo changes)"
echo "  - tsc --noEmit                 (on frontend .ts/.tsx changes)"
echo "The pre-push hook (full CI suite) will run:"
echo "  - cargo build --bins"
echo "  - cargo test"
echo "  - cargo clippy --all-targets -- -D warnings"
echo "  - tsc --noEmit"
echo
echo "Skip for a one-off: git commit --no-verify  /  git push --no-verify"
