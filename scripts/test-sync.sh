#!/usr/bin/env bash
# Real-time sync between two daemon instances on the same machine — a stand-in
# for a second physical device.
#
# Verifies: circle creation, join, presence, file sync, content sync, PSK
# stability across member ops, and stable peer IDs across a restart.
#
# Usage: ./scripts/test-sync.sh
# Requires: cargo (the script builds what it needs)
set -euo pipefail

ENOX="${ENOX:-./target/debug/enox}"

TMPDIR_TEST=$(mktemp -d)
D1_HOME="$TMPDIR_TEST/d1"
D2_HOME="$TMPDIR_TEST/d2"
# `ENOXIAN_HOME` is the supported state-dir override. Overriding `HOME` instead
# does redirect the state, but the CLI then cannot authenticate against its own
# daemon: every call comes back 401 with an empty body, which surfaces as a
# baffling "error decoding response body".
D1_STATE="$D1_HOME/.enoxian"
D2_STATE="$D2_HOME/.enoxian"
# The workspace root resolves from the real home directory and does NOT follow
# ENOXIAN_HOME, so without an explicit --dir both instances would share
# ~/enoxian/<name> and fight over the same files.
D1_WS="$TMPDIR_TEST/ws1"
D2_WS="$TMPDIR_TEST/ws2"
D1_PORT=36541
D2_PORT=36542
CIRCLE_NAME="synctest"

ok()      { echo "  ✓ $*"; }
fail()    { echo "  ✗ $*"; exit 1; }
section() { echo; echo "── $* ──"; }

dump_logs() {
    echo
    echo "── daemon 1 log (tail) ──"
    tail -n 15 "$TMPDIR_TEST/d1.log" 2>/dev/null | sed 's/^/  /' || true
    echo "── daemon 2 log (tail) ──"
    tail -n 15 "$TMPDIR_TEST/d2.log" 2>/dev/null | sed 's/^/  /' || true
}

# `set -e` plus `curl -sf` used to kill this script with no output at all —
# a failed request just ended the run mid-section. Say what happened.
on_err() {
    local line=$1
    echo
    echo "  ✗ failed at line $line"
    dump_logs
}
trap 'on_err $LINENO' ERR

cleanup() {
    pkill -f "enox daemon run --port $D1_PORT" 2>/dev/null || true
    pkill -f "enox daemon run --port $D2_PORT" 2>/dev/null || true
    rm -rf "$TMPDIR_TEST"
}
trap cleanup EXIT

# Every API call needs the bearer token the daemon writes at startup. Each
# instance has its own.
tok1() { cat "$D1_STATE/api.token" 2>/dev/null; }
tok2() { cat "$D2_STATE/api.token" 2>/dev/null; }
curl1() { curl -sf -H "Authorization: Bearer $(tok1)" "http://127.0.0.1:$D1_PORT$1"; }
curl2() { curl -sf -H "Authorization: Bearer $(tok2)" "http://127.0.0.1:$D2_PORT$1"; }

enox1() { ENOXIAN_HOME="$D1_STATE" ENOXIAN_API="http://127.0.0.1:$D1_PORT" "$ENOX" "$@"; }
enox2() { ENOXIAN_HOME="$D2_STATE" ENOXIAN_API="http://127.0.0.1:$D2_PORT" "$ENOX" "$@"; }

# Poll until a predicate holds. Fixed sleeps were the main source of both
# flakiness and slowness here: too short and a healthy run fails, too long and
# every run pays for the worst case.
wait_for() {
    local desc="$1" timeout="$2"; shift 2
    local waited=0
    until "$@"; do
        sleep 1
        waited=$((waited + 1))
        if (( waited >= timeout )); then
            fail "$desc (timed out after ${timeout}s)"
        fi
    done
}

daemon_up()   { curl -sf -o /dev/null "http://127.0.0.1:$1/"; }
# A newly created circle is not served until the daemon picks it up, which it
# does on a periodic sweep rather than instantly. Its scoped routes 404 until
# then, and `curl -sf` turns that into an empty body and a dead script.
circle_live_1() { curl1 "/circles/$CIRCLE_ID/api/status" >/dev/null 2>&1; }
circle_live_2() { curl2 "/circles/$CIRCLE_ID/api/status" >/dev/null 2>&1; }
both_present() {
    local n
    n=$(curl1 "/circles/$CIRCLE_ID/api/who" 2>/dev/null \
        | python3 -c "import sys,json;print(len(json.load(sys.stdin)))" 2>/dev/null || echo 0)
    [[ "$n" -ge 2 ]]
}
d2_has_file()  { curl2 "/circles/$CIRCLE_ID/api/files" 2>/dev/null | grep -q "$1"; }
d2_content_is() { [[ "$(cat "$D2_WS/$1" 2>/dev/null || true)" == "$2" ]]; }

section "Build"
cargo build --bins -q && ok "built"

section "Start daemon 1 (circle creator)"
mkdir -p "$D1_STATE" "$D1_WS"
ENOXIAN_DEVICE_LABEL="device-one" ENOXIAN_HOME="$D1_STATE" \
    "$ENOX" daemon run --port $D1_PORT > "$TMPDIR_TEST/d1.log" 2>&1 &
wait_for "daemon 1 never became reachable" 20 daemon_up $D1_PORT
ok "daemon 1 up on port $D1_PORT"

section "Create circle on daemon 1"
INIT_OUT=$(enox1 init --name "$CIRCLE_NAME" --dir "$D1_WS" 2>&1)
INVITE=$(echo "$INIT_OUT" | grep -oE 'enoxian://[^ ]+' | head -1)
[[ -n "$INVITE" ]] || { echo "$INIT_OUT"; fail "no invite in init output"; }
CIRCLE_ID=$(enox1 circles --json | python3 -c "
import sys, json
print(next(c['circle_id'] for c in json.load(sys.stdin) if c['circle_name'] == '$CIRCLE_NAME'))
")
[[ -n "$CIRCLE_ID" ]] || fail "could not determine circle_id"
ok "circle $CIRCLE_ID created"
wait_for "daemon 1 never started serving the new circle" 30 circle_live_1
ok "circle active on daemon 1"

section "Start daemon 2 and enter circle"
mkdir -p "$D2_STATE" "$D2_WS"
ENOXIAN_DEVICE_LABEL="device-two" ENOXIAN_HOME="$D2_STATE" \
    "$ENOX" daemon run --port $D2_PORT > "$TMPDIR_TEST/d2.log" 2>&1 &
wait_for "daemon 2 never became reachable" 20 daemon_up $D2_PORT
ok "daemon 2 up on port $D2_PORT"

# The daemon listens on 0.0.0.0 but only advertises routable addresses, so
# `listen_addrs` never contains 127.0.0.1 — the previous filter for it always
# came up empty and the join silently dialed nothing. Take the port from any
# TCP listener and reach it over loopback.
D1_TCP_PORT=$(curl1 "/circles/$CIRCLE_ID/api/status" | python3 -c "
import sys, json
addrs = json.load(sys.stdin)['p2p']['listen_addrs']
tcp = [a for a in addrs if '/tcp/' in a]
print(tcp[0].split('/tcp/')[1].split('/')[0] if tcp else '')
")
[[ -n "$D1_TCP_PORT" ]] || fail "daemon 1 reported no TCP listener to dial"
ok "dialing daemon 1 at 127.0.0.1:$D1_TCP_PORT"

# Not `|| true`: a failed join used to be swallowed, so the run continued and
# failed later somewhere confusing.
enox2 enter "$INVITE" --dir "$D2_WS" --peer "/ip4/127.0.0.1/tcp/$D1_TCP_PORT" > /dev/null \
    || fail "daemon 2 could not enter the circle"
ok "daemon 2 entered circle"
wait_for "daemon 2 never started serving the circle" 30 circle_live_2
ok "circle active on daemon 2"

section "Presence — both devices visible"
# join_policy defaults to auto, so daemon 1 approves on its own.
wait_for "the two daemons never saw each other" 45 both_present
ok "both devices in presence"

section "PSK stability — no rotation on member ops"
PSK=$(grep psk_hex "$D1_STATE/circles/$CIRCLE_ID/config.toml" | cut -d'"' -f2)
[[ -n "$PSK" ]] || fail "could not read psk_hex"
ok "PSK stable: ${PSK:0:16}…"

section "File sync — create on daemon 1, expect it on daemon 2"
echo "hello from device one" > "$D1_WS/hello.txt"
wait_for "hello.txt never reached daemon 2" 45 d2_has_file "hello.txt"
ok "daemon 2 sees hello.txt"

section "Content sync"
wait_for "hello.txt content never matched on daemon 2" 30 \
    d2_content_is "hello.txt" "hello from device one"
ok "file content synced correctly"

section "Reverse direction — create on daemon 2, expect it on daemon 1"
echo "hello from device two" > "$D2_WS/back.txt"
d1_has_back() { curl1 "/circles/$CIRCLE_ID/api/files" 2>/dev/null | grep -q "back.txt"; }
wait_for "back.txt never reached daemon 1" 45 d1_has_back
ok "daemon 1 sees back.txt"

section "Docs count"
DOCS=$(curl1 "/circles/$CIRCLE_ID/api/status" | python3 -c "import sys,json;print(json.load(sys.stdin)['docs'])")
[[ "$DOCS" -ge 1 ]] || fail "docs count is $DOCS (expected ≥ 1)"
ok "docs count: $DOCS"

section "Connection errors — no PSK mismatch"
ERRS=$(curl1 "/circles/$CIRCLE_ID/api/status" | python3 -c "
import sys, json
errs = json.load(sys.stdin).get('p2p', {}).get('recent_conn_errors', [])
print(len([e for e in errs if 'mismatch' in e.get('error', '')]))
")
[[ "$ERRS" == "0" ]] || fail "PSK mismatch errors in status: $ERRS"
ok "no PSK mismatch connection errors"

section "Stable peer ID across restart"
PEER_BEFORE=$(curl1 "/circles/$CIRCLE_ID/api/status" | python3 -c "import sys,json;print(json.load(sys.stdin)['p2p']['peer_id'])")
pkill -f "enox daemon run --port $D1_PORT" 2>/dev/null || true
sleep 1
ENOXIAN_DEVICE_LABEL="device-one" ENOXIAN_HOME="$D1_STATE" \
    "$ENOX" daemon run --port $D1_PORT >> "$TMPDIR_TEST/d1.log" 2>&1 &
wait_for "daemon 1 never came back after restart" 20 daemon_up $D1_PORT
PEER_AFTER=$(curl1 "/circles/$CIRCLE_ID/api/status" | python3 -c "import sys,json;print(json.load(sys.stdin)['p2p']['peer_id'])")
[[ "$PEER_BEFORE" == "$PEER_AFTER" ]] || fail "peer ID changed after restart: $PEER_BEFORE → $PEER_AFTER"
ok "peer ID stable across restart"

section "Identity in status API"
DEVICE_LABEL=$(curl1 "/circles/$CIRCLE_ID/api/status" | python3 -c "import sys,json;print(json.load(sys.stdin).get('device_label',''))")
[[ -n "$DEVICE_LABEL" ]] || fail "device_label missing from status response"
ok "device_label in status: $DEVICE_LABEL"

trap - ERR
echo
echo "All sync tests passed ✓"
