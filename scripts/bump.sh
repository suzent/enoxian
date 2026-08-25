#!/usr/bin/env bash
# Prepare a version bump commit on a `chore/release-vX.Y.Z` branch.
#
# Merging that branch is what cuts the release: .github/workflows/tag-release.yml
# tags the merge commit and starts the release pipeline. Normally you do not run
# this by hand — use the "Prepare release" workflow in the Actions tab, which
# runs this script in CI and opens the pull request.
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
REPO_URL="https://github.com/suzent/enoxian"

[[ "$(git -C "$REPO_DIR" branch --show-current)" == "main" ]] || {
    echo "Error: release preparation must start from main"
    exit 1
}
git -C "$REPO_DIR" diff --quiet && git -C "$REPO_DIR" diff --cached --quiet || {
    echo "Error: tracked files have uncommitted changes"
    exit 1
}

# A stale main silently drops CHANGELOG entries: anything merged after your last
# pull stays under [Unreleased] and ships in the *next* release notes while its
# code ships in this one.
if git -C "$REPO_DIR" remote get-url origin >/dev/null 2>&1; then
    git -C "$REPO_DIR" fetch --quiet origin main
    git -C "$REPO_DIR" merge-base --is-ancestor origin/main HEAD || {
        echo "Error: main is behind origin/main — run 'git pull' first"
        exit 1
    }
fi

# ── Read current version ──────────────────────────────────────────────────────
if ! grep -qE '^version[[:space:]]*=[[:space:]]*"[0-9]+\.[0-9]+\.[0-9]+"' "$CARGO_TOML"; then
    echo "Error: could not find version in Cargo.toml"
    exit 1
fi

CURRENT=$(grep -E '^version[[:space:]]*=[[:space:]]*"[0-9]+\.[0-9]+\.[0-9]+"' "$CARGO_TOML" | head -1 \
    | sed -E 's/version[[:space:]]*=[[:space:]]*"([^"]+)"/\1/')

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

# ── Refuse to cut an empty release ────────────────────────────────────────────
# The release notes are this section. An empty one fails the release pipeline
# after five platform builds; catching it here costs nothing.
UNRELEASED="$(awk '
    /^## \[Unreleased\]/ { grab=1; next }
    grab && /^## / { exit }
    grab { print }
' "$CHANGELOG")"
if ! printf '%s' "$UNRELEASED" | grep -q '[^[:space:]]'; then
    echo "Error: the [Unreleased] section is empty — nothing to release"
    exit 1
fi

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

# Insert the new compare link directly below [Unreleased] so the reference list
# stays newest-first, instead of appending it to the end of the file.
awk -v new="$NEW" -v cur="$CURRENT" -v url="$REPO_URL" '
    /^\[Unreleased\]: / {
        print "[Unreleased]: " url "/compare/v" new "...HEAD"
        print "[" new "]: " url "/compare/v" cur "...v" new
        next
    }
    { print }
' "$CHANGELOG" > "$CHANGELOG.tmp"
mv "$CHANGELOG.tmp" "$CHANGELOG"

# ── Update Cargo.toml ─────────────────────────────────────────────────────────
sed -i.bak -E "s/^(version[[:space:]]*=[[:space:]]*)\"[^\"]*\"/\1\"$NEW\"/" "$CARGO_TOML"
rm -f "$CARGO_TOML.bak"

# Keep the user-facing package version synchronized.
sed -i.bak -E "s/^The current package version is \*\*[^*]+\*\*\./The current package version is **$NEW**./" "$README"
rm -f "$README.bak"

# ── Update Cargo.lock ─────────────────────────────────────────────────────────
# Only the workspace member's own version changed, so refresh just that entry
# rather than compiling the tree with `cargo check`.
echo "▶ Updating Cargo.lock..."
(cd "$REPO_DIR" && { cargo update --workspace --offline --quiet 2>/dev/null \
    || cargo update --workspace --quiet; })

# ── Commit only; CI must pass before the tag is created ───────────────────────
echo "▶ Creating release preparation commit..."
git -C "$REPO_DIR" add "$CARGO_TOML" "$REPO_DIR/Cargo.lock" "$CHANGELOG" "$README"
git -C "$REPO_DIR" commit -m "chore: bump version to $NEW"

echo ""
echo "✦ Prepared v$NEW on $BRANCH. Push it and open a PR:"
echo "    git push -u origin $BRANCH"
echo "  Merging that PR tags v$NEW and runs the release pipeline automatically."
