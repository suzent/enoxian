#!/usr/bin/env bash
# Pre-push hook: run the full CI suite before every push.
# Installed by scripts/setup-dev.sh — do not run directly.
#
# Mirrors .github/workflows/ci.yml so a push only reaches GitHub once the
# same checks pass locally. Unlike pre-commit (which lints only changed
# files for speed), this runs the whole suite — push is infrequent and is
# the last gate before CI.
#
# Skip for a one-off: git push --no-verify
set -euo pipefail

YELLOW='\033[0;33m'
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'

step() { echo -e "${YELLOW}▶ $*${NC}"; }
ok()   { echo -e "${GREEN}  ✓ $*${NC}"; }
fail() { echo -e "${RED}  ✗ $*${NC}"; exit 1; }

# CI's clippy is `cargo clippy -- -D warnings`; we add --all-targets so
# test/example code is linted too — a strict superset of what CI runs.
step "cargo build --bins"
cargo build --bins -q 2>&1 || fail "build failed"
ok "build"

step "cargo test"
cargo test -q 2>&1 | tail -6 || fail "tests failed — run 'cargo test'"
ok "tests"

step "cargo clippy --all-targets -- -D warnings"
cargo clippy --all-targets -q -- -D warnings 2>&1 || fail "clippy failed — run 'cargo clippy --all-targets -- -D warnings'"
ok "clippy"

step "frontend typecheck"
(cd frontend && npx tsc -b --noEmit 2>&1) || fail "TypeScript errors — run 'cd frontend && npx tsc -b --noEmit'"
ok "typecheck"

echo -e "${GREEN}pre-push checks passed ✓${NC}"
echo -e "${YELLOW}note: CI runs on Linux + macOS; platform-specific (#[cfg(unix)]) issues may still differ. Keep clippy current with 'rustup update stable'.${NC}"
