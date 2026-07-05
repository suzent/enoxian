#!/usr/bin/env bash
# Bump the version, commit, tag, and push — triggers the GitHub Actions release build.
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

# ── Update Cargo.toml ─────────────────────────────────────────────────────────
sed -i.bak -E "s/^(version\s*=\s*)\"[^\"]*\"/\1\"$NEW\"/" "$CARGO_TOML"
rm -f "$CARGO_TOML.bak"

# ── cargo check to update Cargo.lock ─────────────────────────────────────────
echo "▶ Updating Cargo.lock..."
(cd "$REPO_DIR" && cargo check --quiet 2>/dev/null)

# ── Commit, tag, push ─────────────────────────────────────────────────────────
TAG="v$NEW"
echo "▶ Committing and tagging $TAG..."
git -C "$REPO_DIR" add "$CARGO_TOML" "$REPO_DIR/Cargo.lock"
git -C "$REPO_DIR" commit -m "chore: bump version to $NEW"
git -C "$REPO_DIR" tag "$TAG"
git -C "$REPO_DIR" push
git -C "$REPO_DIR" push origin "$TAG"

echo ""
echo "✦ Released $TAG — GitHub Actions is building the binaries."
echo "  Watch: https://github.com/suzent/enoxian/actions"
echo ""
echo "  Once the build finishes, deploy:"
echo "    ./scripts/rendezvous/deploy-rendezvous.sh <host> --update"
