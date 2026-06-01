#!/usr/bin/env bash
# Run once after cloning to wire up local dev tooling.
# Usage: ./scripts/setup-dev.sh
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"

echo "▶ Installing pre-commit hook..."
cp "$REPO/scripts/pre-commit.sh" "$REPO/.git/hooks/pre-commit"
chmod +x "$REPO/.git/hooks/pre-commit"
echo "  ✓ .git/hooks/pre-commit installed"

echo "▶ Checking rustfmt..."
rustup component add rustfmt clippy 2>/dev/null || true
echo "  ✓ rustfmt + clippy available"

echo "▶ Installing frontend deps..."
(cd "$REPO/frontend" && npm install --silent)
echo "  ✓ frontend deps installed"

echo
echo "Dev setup complete. The pre-commit hook will run:"
echo "  - cargo clippy -- -D warnings  (on .rs / Cargo changes)"
echo "  - cargo test                   (on .rs / Cargo changes)"
echo "  - tsc --noEmit                 (on frontend .ts/.tsx changes)"
echo
echo "Skip for a one-off commit: git commit --no-verify"
