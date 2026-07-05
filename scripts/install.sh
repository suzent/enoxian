#!/usr/bin/env sh
# enoxian quick-install for Linux and macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/suzent/enoxian/main/scripts/install.sh | sh
#
# Downloads the latest (or a pinned) release archive for this OS/arch and
# installs the `enox` and `enoxd` binaries. Override with env vars:
#   ENOXIAN_VERSION=v0.1.4   # pin a release (default: latest)
#   ENOXIAN_BIN_DIR=~/.local/bin   # install dir (default: /usr/local/bin,
#                                  # falling back to ~/.local/bin without sudo)
set -eu

REPO="suzent/enoxian"
VERSION="${ENOXIAN_VERSION:-latest}"

err() { echo "install: $*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# ── Detect OS / arch → release asset name ────────────────────────────────────
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Linux)  os_tag="linux" ;;
  Darwin) os_tag="macos" ;;
  *) err "unsupported OS '$os' — use the Windows installer or build from source" ;;
esac
case "$arch" in
  x86_64|amd64) arch_tag="x86_64" ;;
  arm64|aarch64) arch_tag="aarch64" ;;
  *) err "unsupported arch '$arch'" ;;
esac
asset="enoxian-${os_tag}-${arch_tag}.tar.gz"

# ── Resolve download URL ─────────────────────────────────────────────────────
if [ "$VERSION" = "latest" ]; then
  url="https://github.com/${REPO}/releases/latest/download/${asset}"
else
  url="https://github.com/${REPO}/releases/download/${VERSION}/${asset}"
fi

# ── Pick a download tool ─────────────────────────────────────────────────────
if have curl; then dl() { curl -fsSL "$1" -o "$2"; }
elif have wget; then dl() { wget -qO "$2" "$1"; }
else err "need curl or wget"; fi

# ── Download + extract ───────────────────────────────────────────────────────
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "install: downloading $asset ($VERSION)"
dl "$url" "$tmp/$asset" || err "download failed: $url"
tar -C "$tmp" -xzf "$tmp/$asset" || err "extract failed (unexpected archive)"
[ -f "$tmp/enox" ] && [ -f "$tmp/enoxd" ] || err "archive missing enox/enoxd"
chmod +x "$tmp/enox" "$tmp/enoxd"

# ── Choose an install dir; use sudo only if needed and available ─────────────
bin_dir="${ENOXIAN_BIN_DIR:-}"
if [ -z "$bin_dir" ]; then
  if [ -w /usr/local/bin ]; then bin_dir="/usr/local/bin"
  elif have sudo; then bin_dir="/usr/local/bin"
  else bin_dir="$HOME/.local/bin"; fi
fi
mkdir -p "$bin_dir" 2>/dev/null || true

install_one() {
  if [ -w "$bin_dir" ]; then
    mv "$tmp/$1" "$bin_dir/$1"
  elif have sudo; then
    sudo mv "$tmp/$1" "$bin_dir/$1"
  else
    err "cannot write to $bin_dir and sudo is unavailable — set ENOXIAN_BIN_DIR"
  fi
}
install_one enox
install_one enoxd

echo "install: installed enox and enoxd to $bin_dir"
case ":$PATH:" in
  *":$bin_dir:"*) : ;;
  *) echo "install: note — $bin_dir is not on your PATH; add it to use 'enox'." ;;
esac
echo "install: run 'enox --help' to get started."
