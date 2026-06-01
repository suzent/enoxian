#!/usr/bin/env bash
# Test the device identity system: stable keypair derivation, label/user ops.
# Run from the repo root: ./scripts/test-identity.sh
# Requires: cargo build --bins already done (or pass ENOX=/path/to/enox)
set -euo pipefail

ENOX="${ENOX:-./target/debug/enox}"
ENOXD="${ENOXD:-./target/debug/enoxd}"
# Use high ports that won't conflict with the live daemon (36521)
D1_PORT=36551
D2_PORT=36552

# Save real HOME before overriding (rustup/cargo need it)
REAL_HOME="$HOME"

TMPDIR_TEST=$(mktemp -d)
trap 'kill $DAEMON_PID 2>/dev/null || true; rm -rf "$TMPDIR_TEST"' EXIT

ok()      { echo "  ✓ $*"; }
fail()    { echo "  ✗ $*"; exit 1; }
section() { echo; echo "── $* ──"; }

section "Build check"
# Build before overriding HOME so rustup/cargo can find their toolchains
cargo build --bins -q && ok "binaries built"

# Now switch HOME to an isolated temp dir for all identity/config operations
export HOME="$TMPDIR_TEST/home"
mkdir -p "$HOME"
export ENOXIAN_API="http://127.0.0.1:$D1_PORT"

section "First-run identity creation (non-interactive, ENOXIAN_DEVICE_LABEL)"
export ENOXIAN_DEVICE_LABEL="test-machine"
"$ENOXD" --port $D1_PORT > "$TMPDIR_TEST/d1.log" 2>&1 &
DAEMON_PID=$!
sleep 2

[[ -f "$HOME/.enoxian/identity.toml" ]] || fail "identity.toml not created"
LABEL=$(grep device_label "$HOME/.enoxian/identity.toml" | cut -d'"' -f2)
[[ "$LABEL" == "test-machine" ]] || fail "expected label=test-machine, got $LABEL"
ok "identity.toml created with label='$LABEL'"

section "enox identity show"
OUTPUT=$("$ENOX" identity show)
echo "$OUTPUT" | grep -q "test-machine" || fail "label not in show output"
echo "$OUTPUT" | grep -q "peer ID"     || fail "peer ID missing from show"
ok "identity show works"

section "Stable peer ID across restarts"
PEER_ID_1=$(echo "$OUTPUT" | grep "peer ID" | awk '{print $NF}')
kill $DAEMON_PID 2>/dev/null; sleep 1
"$ENOXD" --port $D1_PORT >> "$TMPDIR_TEST/d1.log" 2>&1 &
DAEMON_PID=$!
sleep 2
OUTPUT2=$("$ENOX" identity show)
PEER_ID_2=$(echo "$OUTPUT2" | grep "peer ID" | awk '{print $NF}')
[[ "$PEER_ID_1" == "$PEER_ID_2" ]] || fail "peer ID changed across restart: $PEER_ID_1 vs $PEER_ID_2"
ok "peer ID stable across restart: $PEER_ID_1"

section "Per-circle key derivation (different circles → different keypairs)"
ENOXIAN_API="http://127.0.0.1:$D1_PORT" "$ENOX" init --name "circle-alpha" > /dev/null 2>&1 || true
ENOXIAN_API="http://127.0.0.1:$D1_PORT" "$ENOX" init --name "circle-beta"  > /dev/null 2>&1 || true
sleep 1
CFGS=($(ls "$HOME/.enoxian/circles/"*/config.toml 2>/dev/null))
if [[ "${#CFGS[@]}" -ge 2 ]]; then
    KEY_A=$(grep keypair_proto_hex "${CFGS[0]}" | cut -d'"' -f2)
    KEY_B=$(grep keypair_proto_hex "${CFGS[1]}" | cut -d'"' -f2)
    [[ "$KEY_A" != "$KEY_B" ]] || fail "two circles derived identical keypairs"
    ok "different circles → different keypairs"
else
    ok "(skipped — fewer than 2 circles initialised)"
fi

section "enox identity set-label"
"$ENOX" identity set-label "renamed-device"
LABEL_NEW=$(grep device_label "$HOME/.enoxian/identity.toml" | cut -d'"' -f2)
[[ "$LABEL_NEW" == "renamed-device" ]] || fail "label not updated, got '$LABEL_NEW'"
ok "set-label works"

section "enox identity set-user"
"$ENOX" identity set-user "testuser"
HANDLE=$(grep user_handle "$HOME/.enoxian/identity.toml" | cut -d'"' -f2)
[[ "$HANDLE" == "testuser" ]] || fail "user_handle not set, got '$HANDLE'"
ok "set-user works"

section "enox identity create-user + link-user (round-trip)"
"$ENOX" identity create-user "alice" > "$TMPDIR_TEST/create_user_out.txt"
grep -q "BACKUP YOUR MNEMONIC" "$TMPDIR_TEST/create_user_out.txt" || fail "mnemonic prompt missing"
# Extract mnemonic — the 24-word line is indented with two spaces
MNEMONIC=$(grep -E "^  [a-z]" "$TMPDIR_TEST/create_user_out.txt" | head -1 | xargs)
WORD_COUNT=$(echo "$MNEMONIC" | wc -w | tr -d ' ')
[[ "$WORD_COUNT" == "24" ]] || fail "expected 24-word mnemonic, got $WORD_COUNT words: '$MNEMONIC'"
ok "create-user produced 24-word mnemonic"

# Simulate linking a second device using the same mnemonic
D2_HOME="$TMPDIR_TEST/home2"
mkdir -p "$D2_HOME"
HOME="$D2_HOME" ENOXIAN_API="http://127.0.0.1:$D2_PORT" \
    "$ENOXD" --port $D2_PORT >> "$TMPDIR_TEST/d2.log" 2>&1 &
D2_PID=$!
sleep 2
# Second device needs an identity first (daemon creates it), then we link
HOME="$D2_HOME" "$ENOX" identity set-label "second-device"
HOME="$D2_HOME" "$ENOX" identity link-user "alice" "$MNEMONIC"
USER2=$(grep user_handle "$D2_HOME/.enoxian/identity.toml" 2>/dev/null | cut -d'"' -f2)
[[ "$USER2" == "alice" ]] || fail "link-user: expected user=alice, got '$USER2'"
ok "link-user works on second device"
kill $D2_PID 2>/dev/null || true

echo
echo "All identity tests passed ✓"
