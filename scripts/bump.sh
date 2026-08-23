#!/usr/bin/env bash
# Prepare a version bump commit. Tagging happens only after this commit is
# merged to main and its required CI checks are green.
#
# Usage:
#   ./scripts/bump.sh patch   # 0.1.0 → 0.1.1
#   ./scripts/bump.sh minor   # 0.1.0 → 0.2.0
#   ./scripts/bump.sh major   # 0.1.0 → 1.0.0
#   ./scripts/bump.sh 0.3.0   # set exact version
set -euo pipefail

PART="${1:-}"
if [[ -z "$PART" ]]; then
    echo "Usage: $0 major|minor|patch|<version>"
    exit 1
fi

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
CARGO_TOML="$REPO_DIR/Cargo.toml"
CHANGELOG="$REPO_DIR/CHANGELOG.md"
README="$REPO_DIR/README.md"

[[ "$(git -C "$REPO_DIR" branch --show-current)" == "main" ]] || {
    echo "Error: release preparation must start from main"
    exit 1
}
git -C "$REPO_DIR" diff --quiet && git -C "$REPO_DIR" diff --cached --quiet || {
    echo "Error: tracked files have uncommitted changes"
    exit 1
}

# ── Read current version ──────────────────────────────────────────────────────
if ! grep -qE '^version\s*=\s*"[0-9]+\.[0-9]+\.[0-9]+"' "$CARGO_TOML"; then
    echo "Error: could not find version in Cargo.toml"
    exit 1
fi

CURRENT=$(grep -E '^version\s*=\s*"[0-9]+\.[0-9]+\.[0-9]+"' "$CARGO_TOML" | head -1 \
    | sed -E 's/version\s*=\s*"([^"]+)"/\1/')

MAJOR=$(echo "$CURRENT" | cut -d. -f1)
MINOR=$(echo "$CURRENT" | cut -d. -f2)
PATCH=$(echo "$CURRENT" | cut -d. -f3)

# ── Compute new version ───────────────────────────────────────────────────────
case "$PART" in
    major) NEW="$((MAJOR + 1)).0.0" ;;
    minor) NEW="${MAJOR}.$((MINOR + 1)).0" ;;
    patch) NEW="${MAJOR}.${MINOR}.$((PATCH + 1))" ;;
    *)
        if [[ "$PART" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
            NEW="$PART"
        else
            echo "Usage: $0 major|minor|patch|<version>"
            exit 1
        fi
        ;;
esac

echo "▶ Bumping $CURRENT → $NEW"
BRANCH="chore/release-v$NEW"
git -C "$REPO_DIR" switch -c "$BRANCH"

# Cut the Unreleased section and update compare links.
DATE="$(date +%Y-%m-%d)"
awk -v header="## [$NEW] — $DATE" '
    !done && $0 == "## [Unreleased]" {
        print
        print ""
        print header
        done=1
        next
    }
    { print }
' "$CHANGELOG" > "$CHANGELOG.tmp"
mv "$CHANGELOG.tmp" "$CHANGELOG"
sed -i.bak -E "s|^\[Unreleased\]: .*|[Unreleased]: https://github.com/suzent/enoxian/compare/v$NEW...HEAD|" "$CHANGELOG"
printf '[%s]: https://github.com/suzent/enoxian/compare/v%s...v%s\n' "$NEW" "$CURRENT" "$NEW" >> "$CHANGELOG"
rm -f "$CHANGELOG.bak"

# ── Update Cargo.toml ─────────────────────────────────────────────────────────
sed -i.bak -E "s/^(version\s*=\s*)\"[^\"]*\"/\1\"$NEW\"/" "$CARGO_TOML"
rm -f "$CARGO_TOML.bak"

# Keep the user-facing package version synchronized.
sed -i.bak -E "s/^The current package version is \*\*[^*]+\*\*\./The current package version is **$NEW**./" "$README"
rm -f "$README.bak"

# ── cargo check to update Cargo.lock ─────────────────────────────────────────
echo "▶ Updating Cargo.lock..."
(cd "$REPO_DIR" && cargo check --quiet 2>/dev/null)

# ── Commit only; CI must pass before the tag is created ───────────────────────
echo "▶ Creating release preparation commit..."
git -C "$REPO_DIR" add "$CARGO_TOML" "$REPO_DIR/Cargo.lock" "$CHANGELOG" "$README"
git -C "$REPO_DIR" commit -m "chore: bump version to $NEW"

echo ""
echo "✦ Prepared v$NEW on $BRANCH. Push it and open a PR:"
echo "    git push -u origin $BRANCH"
echo "  After merge and required CI are green, tag main:"
echo "    git tag -s v$NEW -m 'enoxian v$NEW'"
echo "    git push origin v$NEW"
