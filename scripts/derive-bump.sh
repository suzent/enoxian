#!/usr/bin/env bash
# Derive a version bump level from the commits since the last release tag.
#
# Conventional-commit rules, most significant wins:
#
#   major   a `!` after the type/scope, or `BREAKING CHANGE` in a commit body
#   minor   any `feat` commit
#   patch   anything else
#
# Prints one of major|minor|patch on stdout.
#
# Note on `major`: this repository has never used `!` or `BREAKING CHANGE`, so
# in practice a major bump will not derive itself. That is deliberate — a
# silent major is worse than a manual one — but it does mean a breaking change
# must either adopt the marker or be released by choosing `major` explicitly.
#
# Usage: ./scripts/derive-bump.sh [<since-ref>]
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_DIR"

since="${1:-}"
if [[ -z "$since" ]]; then
    # Most recent v-tag reachable from HEAD; empty on a repository with none.
    since="$(git describe --tags --abbrev=0 --match 'v[0-9]*' 2>/dev/null || true)"
fi

range="HEAD"
if [[ -n "$since" ]]; then
    range="${since}..HEAD"
fi

subjects="$(git log --no-merges --pretty=%s "$range" 2>/dev/null || true)"
bodies="$(git log --no-merges --pretty=%B "$range" 2>/dev/null || true)"

# `type!: …` or `type(scope)!: …`
if grep -qE '^[a-z]+(\([^)]*\))?!:' <<<"$subjects" \
    || grep -q 'BREAKING CHANGE' <<<"$bodies"; then
    echo "major"
    exit 0
fi

if grep -qE '^feat(\([^)]*\))?:' <<<"$subjects"; then
    echo "minor"
    exit 0
fi

echo "patch"
