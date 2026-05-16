#!/usr/bin/env bash
# Deploy enochd to a Linux VPS as a rendezvous server.
#
# Build modes (in order of preference):
#   default           Download latest release binary from GitHub (fastest)
#   --build-on-remote Pipe source into Docker on the VPS and build there
#   --local           Cross-compile locally using cross (Docker)
#
# Usage:
#   ./scripts/deploy-rendezvous.sh user@host [--port PORT] [--build-on-remote] [--local] [--update]
#
# Examples:
#   ./scripts/deploy-rendezvous.sh root@sg.example.com
#   ./scripts/deploy-rendezvous.sh root@sg.example.com --update
#   ./scripts/deploy-rendezvous.sh root@sg.example.com --build-on-remote
set -euo pipefail

if [[ $# -lt 1 || "$1" == --* ]]; then
    echo "Usage: $0 user@host [--port PORT] [--build-on-remote] [--local] [--arch x86_64|aarch64] [--update]"
    exit 1
fi

SSH_TARGET="$1"; shift
PORT=36521
ARCH="x86_64"
UPDATE_ONLY=false
BUILD_ON_REMOTE=false
LOCAL=false
REPO="suzent/enochian"
TOKEN="${GITHUB_TOKEN:-}"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --port)            PORT="$2"; shift 2 ;;
        --arch)            ARCH="$2"; shift 2 ;;
        --build-on-remote) BUILD_ON_REMOTE=true; shift ;;
        --local)           LOCAL=true; shift ;;
        --update)          UPDATE_ONLY=true; shift ;;
        --token)           TOKEN="$2"; shift 2 ;;
        *) echo "Unknown argument: $1"; exit 1 ;;
    esac
done

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
ASSET="enochd-linux-$ARCH"

# Load GITHUB_TOKEN from .env if not already set
if [[ -z "$TOKEN" && -f "$REPO_DIR/.env" ]]; then
    TOKEN=$(grep -E '^\s*GITHUB_TOKEN\s*=' "$REPO_DIR/.env" | head -1 | sed -E 's/^\s*GITHUB_TOKEN\s*=\s*["'"'"']?([^"'"'"'[:space:]]+)["'"'"']?/\1/')
fi

# ── Get binary ────────────────────────────────────────────────────────────────
if $BUILD_ON_REMOTE; then
    echo "▶ Piping source into Docker on ${SSH_TARGET}..."
    tar -czf - \
        --exclude=".git" --exclude="target" --exclude="node_modules" \
        -C "$REPO_DIR" . | \
    ssh "$SSH_TARGET" \
        "docker run --rm -i \
            -v enochian-cargo-cache:/usr/local/cargo/registry \
            -v enochian-out:/out \
            rust:alpine \
            sh -c 'apk add --no-cache musl-dev && mkdir /src && tar -xzf - -C /src && cd /src && cargo build --release --bin enochd && cp target/release/enochd /out/enochd'"

    ssh "$SSH_TARGET" \
        "docker run --rm -v enochian-out:/out busybox cp /out/enochd /tmp/enochd && chmod +x /tmp/enochd"

elif $LOCAL; then
    LINUX_TARGET="${ARCH}-unknown-linux-gnu"
    BINARY="$REPO_DIR/target/$LINUX_TARGET/release/enochd"
    echo "▶ Building enochd for Linux ($LINUX_TARGET)..."
    cd "$REPO_DIR"

    if [[ "$(uname -s)" == "Linux" && "$(uname -m)" == "$ARCH" ]]; then
        cargo build --release --bin enochd
        BINARY="$REPO_DIR/target/release/enochd"
    elif command -v cross &>/dev/null; then
        cross build --release --bin enochd --target "$LINUX_TARGET"
    else
        echo "Error: install cross (cargo install cross) or use --build-on-remote"
        exit 1
    fi

    echo "▶ Uploading..."
    scp "$BINARY" "${SSH_TARGET}:/tmp/enochd"

else
    # ── Download latest GitHub release (default) ─────────────────────────────
    echo "▶ Downloading latest release from github.com/$REPO..."
    RELEASE_JSON=$(curl -fsSL \
        ${TOKEN:+-H "Authorization: Bearer $TOKEN"} \
        "https://api.github.com/repos/$REPO/releases/latest")

    ASSET_ID=$(echo "$RELEASE_JSON" | grep -A5 "\"name\": \"$ASSET\"" | grep '"id"' | head -1 | grep -oE '[0-9]+')
    TAG=$(echo "$RELEASE_JSON" | grep '"tag_name"' | head -1 | cut -d'"' -f4)

    if [[ -z "$ASSET_ID" ]]; then
        echo "Error: asset '$ASSET' not found in latest release."
        echo "Run the release workflow first, or use --build-on-remote."
        exit 1
    fi

    # Use API asset URL to avoid losing auth header on GitHub→S3 redirect
    API_URL="https://api.github.com/repos/$REPO/releases/assets/$ASSET_ID"
    echo "  $TAG: $ASSET"
    if [[ -n "$TOKEN" ]]; then
        ssh "$SSH_TARGET" "curl -fsSL -H 'Authorization: Bearer $TOKEN' -H 'Accept: application/octet-stream' '$API_URL' -o /tmp/enochd && chmod +x /tmp/enochd"
    else
        ssh "$SSH_TARGET" "curl -fsSL -H 'Accept: application/octet-stream' '$API_URL' -o /tmp/enochd && chmod +x /tmp/enochd"
    fi
fi

# ── Install on the VPS ────────────────────────────────────────────────────────
if $UPDATE_ONLY; then
    echo "▶ Updating binary and restarting service..."
    ssh "$SSH_TARGET" "
        set -e
        cp /tmp/enochd /usr/local/bin/enochd
        chmod +x /usr/local/bin/enochd
        systemctl restart enochd-bootstrap
        sleep 1
        systemctl is-active enochd-bootstrap && echo '✦ Service restarted' \
            || { journalctl -u enochd-bootstrap -n 10 --no-pager; exit 1; }
    "
else
    echo "▶ Running setup on $SSH_TARGET..."
    scp "$REPO_DIR/scripts/setup-rendezvous.sh" "$SSH_TARGET:/tmp/setup-rendezvous.sh"
    ssh "$SSH_TARGET" "bash /tmp/setup-rendezvous.sh --port $PORT"
fi
