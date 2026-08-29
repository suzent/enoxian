#!/usr/bin/env bash
# Circle membership operations end to end, against a real daemon.
#
# Every check asserts the *effect* — what the member list says afterwards — not
# the message the command printed. That distinction is the point of this script:
# `enox member add` and `enox member promote` were rejected by the daemon on
# every invocation while printing `✦ done`, because the CLI signed a different
# message than the daemon verified and never looked at the response status.
# Nothing that trusted the output could have caught it.
#
# One daemon, no networking, no timing — fast and deterministic enough for CI.
#
# Usage: ./scripts/test-members.sh
set -euo pipefail

ENOX="${ENOX:-./target/debug/enox}"
TMPDIR_TEST=$(mktemp -d)
STATE="$TMPDIR_TEST/home/.enoxian"
WS="$TMPDIR_TEST/ws"
PORT=36571
CIRCLE_NAME="memtest"
# A syntactically valid peer id that never joins; the daemon only needs it to be
# well-formed to record membership.
PEER="12D3KooWQZ8ehJKtRXgKmZ4rVEbT7c2xAgLYcbcCkPZk9j8fT1Ab"

ok()      { echo "  ✓ $*"; }
fail()    { echo "  ✗ $*"; dump_log; exit 1; }
section() { echo; echo "── $* ──"; }
dump_log() { echo; echo "── daemon log (tail) ──"; tail -n 20 "$TMPDIR_TEST/d.log" 2>/dev/null | sed 's/^/  /' || true; }
trap 'echo; echo "  ✗ failed at line $LINENO"; dump_log' ERR
cleanup() { pkill -f "enox daemon run --port $PORT" 2>/dev/null || true; rm -rf "$TMPDIR_TEST"; }
trap cleanup EXIT

E() { ENOXIAN_HOME="$STATE" ENOXIAN_API="http://127.0.0.1:$PORT" "$ENOX" "$@"; }
# Probe an authenticated API route, not `/`.
#
# `/` serves the WebUI, which the Rust CI job never builds — so it 404s there
# and the daemon looked permanently unreachable. It 404s locally too; the old
# probe only ever passed by accident. Waiting on the token file and a real API
# response tests the thing the script actually needs.
daemon_ready() {
    [[ -f "$STATE/api.token" ]] || return 1
    curl -sf -o /dev/null -H "Authorization: Bearer $(cat "$STATE/api.token")" \
        "http://127.0.0.1:$PORT/circles"
}
members() { E member list --circle "$CIRCLE_ID" 2>/dev/null; }
# Grep the roster rather than the command output: the whole point is to believe
# only observed state.
has_role() { members | grep -q "\[$1\].*$2"; }

section "Build"
cargo build --bins -q && ok "built"

section "Start daemon"
mkdir -p "$STATE" "$WS"
ENOXIAN_HOME="$STATE" "$ENOX" daemon run --port $PORT > "$TMPDIR_TEST/d.log" 2>&1 &
for _ in $(seq 1 40); do daemon_ready && break; sleep 1; done
daemon_ready || fail "daemon never became reachable"
ok "daemon up on port $PORT"

section "Create circle"
E init --name "$CIRCLE_NAME" --dir "$WS" > /dev/null 2>&1 || fail "init failed"
CIRCLE_ID=$(E circles --json | python3 -c "
import sys, json
print(next(c['circle_id'] for c in json.load(sys.stdin) if c['circle_name'] == '$CIRCLE_NAME'))
")
[[ -n "$CIRCLE_ID" ]] || fail "could not determine circle_id"
circle_live() {
    curl -sf -o /dev/null -H "Authorization: Bearer $(cat "$STATE/api.token")" \
        "http://127.0.0.1:$PORT/circles/$CIRCLE_ID/api/status"
}
# A new circle is not served until the daemon's periodic sweep picks it up.
for _ in $(seq 1 40); do circle_live && break; sleep 1; done
circle_live || fail "circle never became active"
ok "circle $CIRCLE_ID active"

section "Founder is admin"
has_role admin "" || fail "founder is not listed as admin"
ok "founder listed as admin"

section "member add"
E member add "$PEER" --owner alice --agent-id alice-mac --circle "$CIRCLE_ID" > /dev/null \
    || fail "member add reported failure"
has_role member "$PEER" || fail "add succeeded but the member is not in the list"
ok "member added and visible in the roster"

section "member promote"
E member promote "$PEER" --circle "$CIRCLE_ID" > /dev/null || fail "member promote reported failure"
has_role admin "$PEER" || fail "promote succeeded but the role did not change"
ok "member promoted to admin"

section "member remove"
E member remove "$PEER" --circle "$CIRCLE_ID" > /dev/null || fail "member remove reported failure"
if members | grep -q "$PEER"; then fail "remove succeeded but the member is still listed"; fi
ok "member removed from the roster"

section "Failures are reported, not swallowed"
# The regression that hid two broken commands: an error response has no
# `status` field, so printing `val["status"].unwrap_or("done")` rendered a
# rejection as success and exited zero.
if OUT=$(E member approve "$PEER" --circle "$CIRCLE_ID" 2>&1); then
    fail "approving a peer with no pending request should have failed, got: $OUT"
fi
case "$OUT" in
    *done*) fail "a rejected request reported success: $OUT" ;;
    *)      ok "rejection surfaced: ${OUT#Error: }" ;;
esac

section "Nonzero exit on failure"
# Disarm the ERR trap: a non-zero exit is the expected result here, not a fault.
trap - ERR
set +e
E member approve "$PEER" --circle "$CIRCLE_ID" > /dev/null 2>&1
RC=$?
set -e
[[ "$RC" -ne 0 ]] || fail "a rejected request exited 0"
ok "exit code $RC"

trap - ERR
echo
echo "All member tests passed ✓"
