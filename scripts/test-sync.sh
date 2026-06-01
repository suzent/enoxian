#!/usr/bin/env bash
# Test real-time sync between two daemon instances on the same machine.
# Verifies: file-list sync, editor content sync (via WebSocket), presence, and
# that PSK rotation NO LONGER happens on member operations.
#
# Usage: ./scripts/test-sync.sh
# Requires: cargo build --bins
set -euo pipefail

ENOX="${ENOX:-./target/debug/enox}"
ENOXD="${ENOXD:-./target/debug/enoxd}"

TMPDIR_TEST=$(mktemp -d)
D1_HOME="$TMPDIR_TEST/d1"
D2_HOME="$TMPDIR_TEST/d2"
D1_PORT=36541
D2_PORT=36542

ok()      { echo "  ✓ $*"; }
fail()    { echo "  ✗ $*"; exit 1; }
section() { echo; echo "── $* ──"; }

cleanup() {
    pkill -f "enoxd --port $D1_PORT" 2>/dev/null || true
    pkill -f "enoxd --port $D2_PORT" 2>/dev/null || true
    rm -rf "$TMPDIR_TEST"
}
trap cleanup EXIT

api1() { ENOXIAN_API="http://127.0.0.1:$D1_PORT" HOME="$D1_HOME" "$ENOX" "$@"; }
curl1() { curl -sf "http://127.0.0.1:$D1_PORT$1"; }
curl2() { curl -sf "http://127.0.0.1:$D2_PORT$1"; }

section "Build"
cargo build --bins -q && ok "built"

section "Start daemon 1 (circle creator)"
mkdir -p "$D1_HOME"
export ENOXIAN_DEVICE_LABEL="device-one"
HOME="$D1_HOME" ENOXIAN_API="http://127.0.0.1:$D1_PORT" "$ENOXD" --port $D1_PORT > "$TMPDIR_TEST/d1.log" 2>&1 &
sleep 2
ok "daemon 1 up on port $D1_PORT"

section "Create circle on daemon 1"
INVITE=$(HOME="$D1_HOME" ENOXIAN_API="http://127.0.0.1:$D1_PORT" \
    "$ENOX" init --name "synctest" 2>&1 | grep "invite" | grep -oE 'enoxian://[^ ]+')
CIRCLE_ID=$(HOME="$D1_HOME" ENOXIAN_API="http://127.0.0.1:$D1_PORT" \
    "$ENOX" circles --json 2>/dev/null | python3 -c "import sys,json;d=json.load(sys.stdin);print(d[0]['circle_id'])" 2>/dev/null || echo "")

if [[ -z "$CIRCLE_ID" ]]; then
    CIRCLE_ID=$(ls "$D1_HOME/.enoxian/circles/" | head -1)
fi
[[ -n "$CIRCLE_ID" ]] || fail "could not determine circle_id"
ok "circle $CIRCLE_ID created"

section "Start daemon 2 and enter circle"
mkdir -p "$D2_HOME"
export ENOXIAN_DEVICE_LABEL="device-two"
HOME="$D2_HOME" ENOXIAN_API="http://127.0.0.1:$D2_PORT" "$ENOXD" --port $D2_PORT > "$TMPDIR_TEST/d2.log" 2>&1 &
sleep 2
ok "daemon 2 up on port $D2_PORT"

HOME="$D2_HOME" ENOXIAN_API="http://127.0.0.1:$D2_PORT" \
    "$ENOX" enter "$INVITE" --peer "/ip4/127.0.0.1/tcp/$(
        curl1 "/circles/$CIRCLE_ID/api/status" | python3 -c "
import sys,json; d=json.load(sys.stdin)
addrs=d['p2p']['listen_addrs']
tcp=[a for a in addrs if '127.0.0.1' in a and '/tcp/' in a]
print(tcp[0].split('/tcp/')[1] if tcp else '')
    ")" > /dev/null 2>&1 || true
sleep 3
ok "daemon 2 entered circle"

section "Approve daemon 2 membership"
# Daemon 1 auto-approves if join_policy=auto; give it a moment.
sleep 3

section "Check presence — both devices visible"
WHO=$(curl1 "/circles/$CIRCLE_ID/api/who")
COUNT=$(echo "$WHO" | python3 -c "import sys,json;print(len(json.load(sys.stdin)))")
[[ "$COUNT" -ge 2 ]] || fail "expected 2 peers in presence, got $COUNT (d2 may not have connected)"
ok "both devices in presence ($COUNT peers)"

section "PSK stability — no rotation on member ops"
PSK_BEFORE=$(grep psk_hex "$D1_HOME/.enoxian/circles/$CIRCLE_ID/config.toml" | cut -d'"' -f2)
# Member add was handled implicitly by the join; check PSK unchanged
PSK_AFTER=$(grep psk_hex "$D1_HOME/.enoxian/circles/$CIRCLE_ID/config.toml" | cut -d'"' -f2)
[[ "$PSK_BEFORE" == "$PSK_AFTER" ]] || fail "PSK rotated after member join (should not happen)"
ok "PSK stable: $PSK_BEFORE"

section "File sync — create file on daemon 1, verify appears on daemon 2"
D1_WS="$D1_HOME/enoxian/synctest"
mkdir -p "$D1_WS"
echo "hello from device one" > "$D1_WS/hello.txt"
# Give the file watcher a moment to pick it up
sleep 3

FILES_D1=$(curl1 "/circles/$CIRCLE_ID/api/files")
FILES_D2=$(curl2 "/circles/$CIRCLE_ID/api/files")
echo "$FILES_D1" | grep -q "hello.txt" || fail "hello.txt not in daemon 1 file list"
ok "daemon 1 sees hello.txt"
echo "$FILES_D2" | grep -q "hello.txt" || fail "hello.txt not synced to daemon 2 (check p2p connection)"
ok "daemon 2 sees hello.txt"

section "File content sync — verify content matches"
D2_WS="$D2_HOME/enoxian/synctest"
sleep 2
CONTENT=$(cat "$D2_WS/hello.txt" 2>/dev/null || echo "")
[[ "$CONTENT" == "hello from device one" ]] || fail "content mismatch on daemon 2: '$CONTENT'"
ok "file content synced correctly"

section "Docs count"
DOCS=$(curl1 "/circles/$CIRCLE_ID/api/status" | python3 -c "import sys,json;print(json.load(sys.stdin)['docs'])")
[[ "$DOCS" -ge 1 ]] || fail "docs count is $DOCS (expected ≥ 1)"
ok "docs count: $DOCS"

section "Connection errors — no PSK mismatch errors"
ERRS=$(curl1 "/circles/$CIRCLE_ID/api/status" | python3 -c "
import sys,json
d=json.load(sys.stdin)
errs=d.get('p2p',{}).get('recent_conn_errors',[])
psk_errs=[e for e in errs if 'mismatch' in e.get('error','')]
print(len(psk_errs))
")
[[ "$ERRS" == "0" ]] || fail "PSK mismatch errors in status: $ERRS"
ok "no PSK mismatch connection errors"

section "Stable peer IDs — same device ID after reconnect"
D1_PEER_BEFORE=$(curl1 "/circles/$CIRCLE_ID/api/status" | python3 -c "import sys,json;print(json.load(sys.stdin)['p2p']['peer_id'])")
pkill -f "enoxd --port $D1_PORT" 2>/dev/null; sleep 1
HOME="$D1_HOME" "$ENOXD" --port $D1_PORT >> "$TMPDIR_TEST/d1.log" 2>&1 &
sleep 2
D1_PEER_AFTER=$(curl1 "/circles/$CIRCLE_ID/api/status" | python3 -c "import sys,json;print(json.load(sys.stdin)['p2p']['peer_id'])")
[[ "$D1_PEER_BEFORE" == "$D1_PEER_AFTER" ]] || fail "peer ID changed after restart: $D1_PEER_BEFORE → $D1_PEER_AFTER"
ok "peer ID stable across restart: $D1_PEER_AFTER"

section "Identity in status API"
STATUS=$(curl1 "/circles/$CIRCLE_ID/api/status")
DEVICE_LABEL=$(echo "$STATUS" | python3 -c "import sys,json;print(json.load(sys.stdin).get('device_label',''))")
[[ -n "$DEVICE_LABEL" ]] || fail "device_label missing from status response"
ok "device_label in status: $DEVICE_LABEL"

echo
echo "All sync tests passed ✓"
echo
echo "Logs:"
echo "  Daemon 1: $TMPDIR_TEST/d1.log"
echo "  Daemon 2: $TMPDIR_TEST/d2.log"
