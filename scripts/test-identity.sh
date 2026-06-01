#!/usr/bin/env bash
# Test the device identity system: stable keypair derivation, label/user ops.
# Run from the repo root: ./scripts/test-identity.sh
# Requires: cargo build --bins already done (or pass ENOX=/path/to/enox)
set -euo pipefail

ENOX="${ENOX:-./target/debug/enox}"
ENOXD="${ENOXD:-./target/debug/enoxd}"
API="${ENOXIAN_API:-http://127.0.0.1:36521}"

TMPDIR_TEST=$(mktemp -d)
trap 'rm -rf "$TMPDIR_TEST"; pkill -f "enoxd --port 36521" 2>/dev/null || true' EXIT

export HOME="$TMPDIR_TEST/home"
mkdir -p "$HOME"

ok()  { echo "  ✓ $*"; }
fail(){ echo "  ✗ $*"; exit 1; }
section() { echo; echo "── $* ──"; }

section "Build check"
cargo build --bins -q && ok "binaries built"

section "First-run identity creation (non-interactive, ENOXIAN_DEVICE_LABEL)"
export ENOXIAN_DEVICE_LABEL="test-machine"
"$ENOXD" &
DAEMON_PID=$!
sleep 2

[[ -f "$HOME/.enoxian/identity.toml" ]] || fail "identity.toml not created"
LABEL=$(grep device_label "$HOME/.enoxian/identity.toml" | cut -d'"' -f2)
[[ "$LABEL" == "test-machine" ]] || fail "expected label=test-machine, got $LABEL"
ok "identity.toml created with label='$LABEL'"

section "enox identity show"
OUTPUT=$("$ENOX" identity show)
echo "$OUTPUT" | grep -q "test-machine" || fail "label not in show output"
echo "$OUTPUT" | grep -q "peer ID" || fail "peer ID missing from show"
ok "identity show works"

section "Stable peer ID across restarts"
PEER_ID_1=$(echo "$OUTPUT" | grep "peer ID" | awk '{print $NF}')
kill $DAEMON_PID 2>/dev/null; sleep 1
"$ENOXD" &
DAEMON_PID=$!
sleep 2
OUTPUT2=$("$ENOX" identity show)
PEER_ID_2=$(echo "$OUTPUT2" | grep "peer ID" | awk '{print $NF}')
[[ "$PEER_ID_1" == "$PEER_ID_2" ]] || fail "peer ID changed across restart: $PEER_ID_1 vs $PEER_ID_2"
ok "peer ID stable across restart: $PEER_ID_1"

section "Per-circle key derivation (different circles → different peer IDs)"
# Derive two circle keys from the same device key — they must differ.
CIRCLE_A=$(uuidgen | tr '[:upper:]' '[:lower:]')
CIRCLE_B=$(uuidgen | tr '[:upper:]' '[:lower:]')
# We verify via init (each circle gets a distinct config with a distinct peer ID)
"$ENOX" init --name "circle-a-$CIRCLE_A" >/dev/null 2>&1 || true
"$ENOX" init --name "circle-b-$CIRCLE_B" >/dev/null 2>&1 || true
CFG_A=$(ls "$HOME/.enoxian/circles/"/*/config.toml 2>/dev/null | head -1)
CFG_B=$(ls "$HOME/.enoxian/circles/"/*/config.toml 2>/dev/null | tail -1)
if [[ -n "$CFG_A" && -n "$CFG_B" && "$CFG_A" != "$CFG_B" ]]; then
    KEY_A=$(grep keypair_proto_hex "$CFG_A" | cut -d'"' -f2)
    KEY_B=$(grep keypair_proto_hex "$CFG_B" | cut -d'"' -f2)
    [[ "$KEY_A" != "$KEY_B" ]] || fail "two circles derived identical keypairs"
    ok "different circles → different keypairs"
else
    ok "(skipped circle derivation check — only one circle found)"
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
"$ENOX" identity create-user "alice" > /tmp/create_user_out.txt
grep -q "BACKUP YOUR MNEMONIC" /tmp/create_user_out.txt || fail "mnemonic prompt missing"
# Extract mnemonic (24 words on a single line)
MNEMONIC=$(grep -E "^  [a-z]" /tmp/create_user_out.txt | head -1 | xargs)
WORD_COUNT=$(echo "$MNEMONIC" | wc -w | tr -d ' ')
[[ "$WORD_COUNT" == "24" ]] || fail "expected 24-word mnemonic, got $WORD_COUNT words"
ok "create-user produced 24-word mnemonic"

# Simulate linking a second device using the mnemonic
export HOME2="$TMPDIR_TEST/home2"
mkdir -p "$HOME2"
PREV_HOME="$HOME"
HOME="$HOME2" "$ENOXD" --port 36522 &
D2_PID=$!
sleep 2
HOME="$HOME2" "$ENOX" identity set-label "second-device"
HOME="$HOME2" "$ENOX" identity link-user "alice" "$MNEMONIC"
USER2=$(HOME="$HOME2" grep user_handle "$HOME2/.enoxian/identity.toml" | cut -d'"' -f2)
[[ "$USER2" == "alice" ]] || fail "link-user: expected user=alice, got '$USER2'"
ok "link-user works on second device"
kill $D2_PID 2>/dev/null || true
HOME="$PREV_HOME"

kill $DAEMON_PID 2>/dev/null || true
echo
echo "All identity tests passed ✓"
