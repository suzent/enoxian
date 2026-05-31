#!/usr/bin/env bash
# Run this on any machine to pull latest code, rebuild, and restart enoxd.
# Usage: ./scripts/dev-sync.sh
set -e

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_DIR"

echo "▶ Pulling latest..."
git pull

echo "▶ Building..."
cargo build --bins

echo "▶ Restarting enoxd..."
pkill -f "enoxd" 2>/dev/null || true
sleep 1
nohup ./target/debug/enoxd > ~/.enoxian/daemon.log 2>&1 &
echo "✓ enoxd started (pid $!, log: ~/.enoxian/daemon.log)"
