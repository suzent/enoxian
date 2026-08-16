#!/usr/bin/env sh
# Install enoxian release binaries on Linux or macOS.
#
#   curl -fsSL https://github.com/suzent/enoxian/releases/latest/download/install.sh | sh
#   curl -fsSL https://github.com/suzent/enoxian/releases/latest/download/install.sh | sh -s -- --version v0.3.0
set -eu

REPO="suzent/enoxian"
VERSION="${ENOXIAN_VERSION:-latest}"
BIN_DIR="${ENOXIAN_BIN_DIR:-}"
DOWNLOAD_BASE="${ENOXIAN_DOWNLOAD_BASE:-}"

usage() {
  cat <<'EOF'
Install enoxian on Linux or macOS.

Usage: install.sh [--version VERSION] [--bin-dir DIRECTORY] [--help]

Options:
  --version VERSION   Install a release such as v0.3.0 (default: latest)
  --bin-dir DIRECTORY Install into DIRECTORY
  -h, --help          Show this help

Environment equivalents: ENOXIAN_VERSION, ENOXIAN_BIN_DIR
EOF
}

err() { echo "enoxian installer: error: $*" >&2; exit 1; }
info() { echo "enoxian installer: $*"; }
have() { command -v "$1" >/dev/null 2>&1; }

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version) [ "$#" -ge 2 ] || err "--version needs a value"; VERSION="$2"; shift 2 ;;
    --bin-dir) [ "$#" -ge 2 ] || err "--bin-dir needs a value"; BIN_DIR="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) err "unknown option '$1' (try --help)" ;;
  esac
done

case "$VERSION" in
  latest) ;;
  v*) ;;
  *) VERSION="v$VERSION" ;;
esac

os="${ENOXIAN_OS:-$(uname -s)}"
arch="${ENOXIAN_ARCH:-$(uname -m)}"
case "$os" in
  Linux) os_tag="linux" ;;
  Darwin) os_tag="macos" ;;
  *) err "unsupported OS '$os'; use install.ps1 on Windows" ;;
esac
case "$arch" in
  x86_64|amd64) arch_tag="x86_64" ;;
  arm64|aarch64) arch_tag="aarch64" ;;
  *) err "unsupported architecture '$arch'" ;;
esac
asset="enoxian-${os_tag}-${arch_tag}.tar.gz"

if [ -z "$DOWNLOAD_BASE" ]; then
  if [ "$VERSION" = "latest" ]; then
    DOWNLOAD_BASE="https://github.com/${REPO}/releases/latest/download"
  else
    DOWNLOAD_BASE="https://github.com/${REPO}/releases/download/${VERSION}"
  fi
fi

if [ -z "$BIN_DIR" ]; then
  if [ -d /usr/local/bin ] && [ -w /usr/local/bin ]; then
    BIN_DIR="/usr/local/bin"
  else
    BIN_DIR="${HOME:?HOME is not set}/.local/bin"
  fi
fi

tmp="$(mktemp -d 2>/dev/null || mktemp -d -t enoxian)"
committed=0
changed=0
had_enox=0
had_enoxd=0

cleanup() {
  code=$?
  trap - EXIT HUP INT TERM
  if [ "$changed" -eq 1 ] && [ "$committed" -eq 0 ]; then
    info "installation failed; restoring the previous installation"
    if [ "$had_enox" -eq 1 ]; then cp "$tmp/backup/enox" "$BIN_DIR/enox"; else rm -f "$BIN_DIR/enox"; fi
    if [ "$had_enoxd" -eq 1 ]; then cp "$tmp/backup/enoxd" "$BIN_DIR/enoxd"; else rm -f "$BIN_DIR/enoxd"; fi
  fi
  rm -rf "$tmp"
  exit "$code"
}
trap cleanup EXIT
trap 'exit 130' HUP INT TERM

download() {
  case "$1" in
    https://*|http://127.0.0.1:*|http://localhost:*) ;;
    *) err "refusing unsupported download URL '$1'" ;;
  esac
  if have curl; then
    case "$1" in
      https://*) curl --proto '=https' --tlsv1.2 -fsSL "$1" -o "$2" ;;
      http://127.0.0.1:*|http://localhost:*) curl -fsSL "$1" -o "$2" ;;
    esac
  elif have wget; then
    wget -qO "$2" "$1"
  else
    err "curl or wget is required"
  fi
}

info "detected ${os_tag}/${arch_tag}"
info "downloading $asset ($VERSION)"
download "$DOWNLOAD_BASE/$asset" "$tmp/$asset" || err "download failed: $DOWNLOAD_BASE/$asset"
download "$DOWNLOAD_BASE/SHA256SUMS" "$tmp/SHA256SUMS" || err "SHA256SUMS is unavailable"

expected="$(awk -v name="$asset" '$2 == name || $2 == "*" name { print $1; exit }' "$tmp/SHA256SUMS")"
[ -n "$expected" ] || err "SHA256SUMS has no entry for $asset"
if have sha256sum; then
  actual="$(sha256sum "$tmp/$asset" | awk '{print $1}')"
elif have shasum; then
  actual="$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')"
else
  err "sha256sum or shasum is required to verify the release"
fi
[ "$actual" = "$expected" ] || err "checksum mismatch for $asset"
info "checksum verified"

tar -C "$tmp" -xzf "$tmp/$asset" || err "failed to extract $asset"
[ -f "$tmp/enox" ] && [ -f "$tmp/enoxd" ] || err "archive is missing enox or enoxd"
chmod +x "$tmp/enox" "$tmp/enoxd"
staged_version="$($tmp/enox --version 2>/dev/null)" || err "downloaded enox failed its pre-install check"
$tmp/enoxd --version >/dev/null 2>&1 || err "downloaded enoxd failed its pre-install check"
if [ "$VERSION" != "latest" ]; then
  case "$staged_version" in
    *"${VERSION#v}"*) ;;
    *) err "downloaded version '$staged_version' does not match requested $VERSION" ;;
  esac
fi

mkdir -p "$BIN_DIR" || err "cannot create $BIN_DIR; choose a writable directory with --bin-dir"
[ -w "$BIN_DIR" ] || err "$BIN_DIR is not writable; choose another directory with --bin-dir"
mkdir "$tmp/backup"
if [ -f "$BIN_DIR/enox" ]; then cp "$BIN_DIR/enox" "$tmp/backup/enox"; had_enox=1; fi
if [ -f "$BIN_DIR/enoxd" ]; then cp "$BIN_DIR/enoxd" "$tmp/backup/enoxd"; had_enoxd=1; fi

for binary in enox enoxd; do
  cp "$tmp/$binary" "$BIN_DIR/.${binary}.new.$$"
  chmod 0755 "$BIN_DIR/.${binary}.new.$$"
  mv -f "$BIN_DIR/.${binary}.new.$$" "$BIN_DIR/$binary"
  changed=1
done

"$BIN_DIR/enox" --version >/dev/null 2>&1 || err "installed enox failed its post-install check"
"$BIN_DIR/enoxd" --version >/dev/null 2>&1 || err "installed enoxd failed its post-install check"
committed=1

info "installed $staged_version"
info "binaries: $BIN_DIR/enox and $BIN_DIR/enoxd"
case ":$PATH:" in
  *":$BIN_DIR:"*) info "next: run 'enox init --name my-project'" ;;
  *)
    info "$BIN_DIR is not on PATH"
    info "add this line to your shell profile, then open a new terminal:"
    echo "  export PATH=\"$BIN_DIR:\$PATH\""
    ;;
esac
if have pgrep && pgrep -x enoxd >/dev/null 2>&1; then
  info "an older enoxd process is still running; restart it to use the new version"
fi
