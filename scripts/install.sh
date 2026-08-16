#!/usr/bin/env sh
# Install enoxian release binaries on Linux or macOS.
#
#   curl -fsSL https://github.com/suzent/enoxian/releases/latest/download/install.sh | sh
#
# Options:
#   ENOXIAN_VERSION=v0.3.0
#   ENOXIAN_BIN_DIR=$HOME/.local/bin
set -eu

REPO="suzent/enoxian"
VERSION="${ENOXIAN_VERSION:-latest}"

err() { echo "install: $*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Linux) os_tag="linux" ;;
  Darwin) os_tag="macos" ;;
  *) err "unsupported OS '$os' — use install.ps1 on Windows" ;;
esac
case "$arch" in
  x86_64|amd64) arch_tag="x86_64" ;;
  arm64|aarch64) arch_tag="aarch64" ;;
  *) err "unsupported architecture '$arch'" ;;
esac
asset="enoxian-${os_tag}-${arch_tag}.tar.gz"

if [ "$VERSION" = "latest" ]; then
  base="https://github.com/${REPO}/releases/latest/download"
else
  base="https://github.com/${REPO}/releases/download/${VERSION}"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT HUP INT TERM

download() {
  if have curl; then
    curl -fsSL "$1" -o "$2"
  elif have wget; then
    wget -qO "$2" "$1"
  else
    err "need curl or wget"
  fi
}

echo "install: downloading $asset ($VERSION)"
download "$base/$asset" "$tmp/$asset" || err "download failed: $base/$asset"
download "$base/SHA256SUMS" "$tmp/SHA256SUMS" || err "SHA256SUMS is unavailable"

expected="$(awk -v name="$asset" '$2 == name || $2 == "*" name { print $1; exit }' "$tmp/SHA256SUMS")"
[ -n "$expected" ] || err "SHA256SUMS has no entry for $asset"
if have sha256sum; then
  actual="$(sha256sum "$tmp/$asset" | awk '{print $1}')"
elif have shasum; then
  actual="$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')"
else
  err "need sha256sum or shasum to verify the release"
fi
[ "$actual" = "$expected" ] || err "checksum mismatch for $asset"
echo "install: checksum verified"

tar -C "$tmp" -xzf "$tmp/$asset" || err "failed to extract $asset"
[ -f "$tmp/enox" ] && [ -f "$tmp/enoxd" ] || err "archive missing enox or enoxd"
chmod +x "$tmp/enox" "$tmp/enoxd"

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
    sudo install -m 0755 "$tmp/$1" "$bin_dir/$1"
  else
    err "cannot write to $bin_dir and sudo is unavailable — set ENOXIAN_BIN_DIR"
  fi
}
install_one enox
install_one enoxd

"$bin_dir/enox" --version >/dev/null || err "installed enox failed its smoke test"
"$bin_dir/enoxd" --version >/dev/null || err "installed enoxd failed its smoke test"

echo "install: installed enox and enoxd to $bin_dir"
case ":$PATH:" in
  *":$bin_dir:"*) : ;;
  *) echo "install: note — $bin_dir is not on PATH" ;;
esac
