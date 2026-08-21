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
ENABLE_SERVICE="${ENOXIAN_ENABLE_SERVICE:-0}"

usage() {
  cat <<'EOF'
Install enoxian on Linux or macOS.

Usage: install.sh [--version VERSION] [--bin-dir DIRECTORY] [--enable-service] [--help]

Options:
  --version VERSION   Install a release such as v0.3.0 (default: latest)
  --bin-dir DIRECTORY Install into DIRECTORY
  --enable-service    Start Enoxian now and automatically at login
  -h, --help          Show this help

Environment equivalents: ENOXIAN_VERSION, ENOXIAN_BIN_DIR, ENOXIAN_ENABLE_SERVICE
EOF
}

err() { echo "enoxian installer: error: $*" >&2; exit 1; }
info() { echo "enoxian installer: $*"; }
have() { command -v "$1" >/dev/null 2>&1; }

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version) [ "$#" -ge 2 ] || err "--version needs a value"; VERSION="$2"; shift 2 ;;
    --bin-dir) [ "$#" -ge 2 ] || err "--bin-dir needs a value"; BIN_DIR="$2"; shift 2 ;;
    --enable-service) ENABLE_SERVICE=1; shift ;;
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

cleanup() {
  code=$?
  trap - EXIT HUP INT TERM
  if [ "$changed" -eq 1 ] && [ "$committed" -eq 0 ]; then
    info "installation failed; restoring the previous installation"
    if [ "$had_enox" -eq 1 ]; then cp "$tmp/backup/enox" "$BIN_DIR/enox"; else rm -f "$BIN_DIR/enox"; fi
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
[ -f "$tmp/enox" ] || err "archive is missing enox"
chmod +x "$tmp/enox"
staged_version="$($tmp/enox --version 2>/dev/null)" || err "downloaded enox failed its pre-install check"
if [ "$VERSION" != "latest" ]; then
  case "$staged_version" in
    *"${VERSION#v}"*) ;;
    *) err "downloaded version '$staged_version' does not match requested $VERSION" ;;
  esac
fi

mkdir -p "$BIN_DIR" || err "cannot create $BIN_DIR; choose a writable directory with --bin-dir"
[ -w "$BIN_DIR" ] || err "$BIN_DIR is not writable; choose another directory with --bin-dir"
mkdir "$tmp/backup"
if [ -x "$BIN_DIR/enox" ]; then
  "$BIN_DIR/enox" stop >/dev/null 2>&1 || true
fi
if [ -f "$BIN_DIR/enox" ]; then cp "$BIN_DIR/enox" "$tmp/backup/enox"; had_enox=1; fi

cp "$tmp/enox" "$BIN_DIR/.enox.new.$$"
chmod 0755 "$BIN_DIR/.enox.new.$$"
mv -f "$BIN_DIR/.enox.new.$$" "$BIN_DIR/enox"
changed=1

"$BIN_DIR/enox" --version >/dev/null 2>&1 || err "installed enox failed its post-install check"
"$BIN_DIR/enox" update --record-stable >/dev/null 2>&1 || err "failed to record the stable update channel"
committed=1
rm -f "$BIN_DIR/enoxd"

info "installed $staged_version"
info "binary: $BIN_DIR/enox"
if [ "$ENABLE_SERVICE" = "1" ]; then
  "$BIN_DIR/enox" service install --force || err "enox installed, but login service setup failed"
else
  info "optional: run 'enox service install' to start automatically when you sign in"
fi
info "agents: adapters require system Node.js 22+ with npm"
case ":$PATH:" in
  *":$BIN_DIR:"*) info "next: run 'enox init --name my-project'" ;;
  *)
    info "$BIN_DIR is not on PATH"
    info "add this line to your shell profile, then open a new terminal:"
    echo "  export PATH=\"$BIN_DIR:\$PATH\""
    ;;
esac
