#!/usr/bin/env bash
# Pre-commit hook: lint + test before every commit.
# Installed by scripts/setup-dev.sh — do not run directly.
#
# Catches clippy failures locally so CI doesn't surprise you.
# Skip for a one-off: git commit --no-verify
set -euo pipefail

YELLOW='\033[0;33m'
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'

step() { echo -e "${YELLOW}▶ $*${NC}"; }
ok()   { echo -e "${GREEN}  ✓ $*${NC}"; }
fail() { echo -e "${RED}  ✗ $*${NC}"; exit 1; }

RUST_CHANGED=$(git diff --cached --name-only | grep -E '\.rs$|Cargo\.(toml|lock)' || true)
FRONTEND_CHANGED=$(git diff --cached --name-only | grep -E '^frontend/.*\.(ts|tsx)$' || true)

if [[ -n "$RUST_CHANGED" ]]; then
    step "cargo clippy"
    cargo clippy -- -D warnings -q 2>&1 || fail "clippy failed — run 'cargo clippy -- -D warnings' to see errors"
    ok "clippy"

    step "cargo test"
    cargo test -q 2>&1 | tail -6 || fail "tests failed"
    ok "tests"
fi

if [[ -n "$FRONTEND_CHANGED" ]]; then
    step "frontend typecheck"
    (cd frontend && npx tsc -b --noEmit -q 2>&1) || fail "TypeScript errors — run 'cd frontend && npx tsc -b --noEmit'"
    ok "typecheck"
fi

echo -e "${GREEN}pre-commit checks passed ✓${NC}"
